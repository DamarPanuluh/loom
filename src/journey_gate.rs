//! One-shot host-mediated human gates for compiled Journey proofs.
//!
//! This module owns the policy and continuation seam only. It never opens a
//! Loom graph, settles a proof, or appends authority to the journal. The
//! Journey runtime supplies a trusted runtime-temporary root, persists its
//! opaque continuation through the returned paths, and later journals only the
//! [`AuthorityReceipt`] after a successful resume.

use crate::ratification::HumanDecision;
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const PENDING_HUMAN_SCHEMA: &str = "loom.journey.pending-human/v1";
pub const CONTINUATION_CAPSULE_SCHEMA: &str = "loom.journey-continuation/v1";
pub const AUTHORITY_RECEIPT_SCHEMA: &str = "loom.journey-human-decision/v1";

const TOKEN_PREFIX: &str = "jgt1_";
const TOKEN_RANDOM_HEX_LEN: usize = 64;
const STORE_DIR: &str = "journey-gates-v1";
const PENDING_DIR: &str = "pending";
const CLAIMED_DIR: &str = "claimed";
const CAPSULE_FILE: &str = "capsule.json";
const WORKSPACE_DIR: &str = "workspace";
const RUNTIME_STATE_FILE: &str = "runtime-state.json";
// Continuations have no shorter protocol expiry. Keep a generous window for a
// human response while bounding abandoned runtime snapshots.
const CONTINUATION_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The current subject whose meaning the human is deciding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateSubject {
    pub kind: String,
    pub id: String,
    pub hash: String,
}

/// Every fact that makes a pending human question current.
///
/// A resume is stale when any field differs. `prompt_hash` is the digest of the
/// normalized question, recommendation, and ordered option list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateBinding {
    pub journey_id: String,
    pub profile: String,
    pub journey_hash: String,
    pub surface_hash: String,
    pub step_id: String,
    pub step_index: usize,
    pub subject: GateSubject,
    pub prompt_hash: String,
}

impl GateBinding {
    pub fn validate(&self) -> Result<()> {
        crate::journey::validate_stable_id("journey", &self.journey_id)?;
        crate::journey::validate_stable_id("Journey profile", &self.profile)?;
        crate::journey::validate_stable_id("Journey step", &self.step_id)?;
        validate_text("gate subject kind", &self.subject.kind)?;
        validate_text("gate subject id", &self.subject.id)?;
        validate_hash("Journey hash", &self.journey_hash)?;
        validate_hash("surface hash", &self.surface_hash)?;
        validate_hash("gate subject hash", &self.subject.hash)?;
        validate_sha256("prompt hash", &self.prompt_hash)
    }
}

/// One human-facing choice. Ordering is preserved exactly because the host
/// presents this list conversationally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanOption {
    pub id: String,
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub free_form: bool,
}

impl HumanOption {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        free_form: bool,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            free_form,
        }
    }
}

/// The structured question emitted by the presentation step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanPrompt {
    pub question: String,
    pub recommendation: String,
    pub options: Vec<HumanOption>,
}

impl HumanPrompt {
    pub fn new(
        question: impl Into<String>,
        recommendation: impl Into<String>,
        options: Vec<HumanOption>,
    ) -> Result<Self> {
        Self {
            question: question.into(),
            recommendation: recommendation.into(),
            options,
        }
        .normalized()
    }

    /// Normalize insignificant surrounding whitespace while retaining option
    /// order and all substantive human-facing wording.
    pub fn normalized(&self) -> Result<Self> {
        let question = normalize_text("human gate question", &self.question)?;
        let recommendation = normalize_text("human gate recommendation", &self.recommendation)?;
        if !(2..=3).contains(&self.options.len()) {
            bail!(
                "human gate must offer two or three choices (found {})",
                self.options.len()
            );
        }
        let mut ids = BTreeSet::new();
        let mut options = Vec::with_capacity(self.options.len());
        for option in &self.options {
            let id = option.id.trim().to_string();
            crate::journey::validate_stable_id("human gate option", &id)?;
            if !ids.insert(id.clone()) {
                bail!("human gate repeats option id '{id}'");
            }
            options.push(HumanOption {
                id,
                label: normalize_text("human gate option label", &option.label)?,
                description: normalize_text("human gate option description", &option.description)?,
                free_form: option.free_form,
            });
        }
        Ok(Self {
            question,
            recommendation,
            options,
        })
    }

    /// Stable SHA-256 over the normalized prompt. Callers place this in
    /// [`GateBinding::prompt_hash`] before issuing the continuation.
    pub fn digest(&self) -> Result<String> {
        let normalized = self.normalized()?;
        Ok(sha256_hex(&serde_json::to_vec(&normalized)?))
    }
}

/// Host-facing pause result. It deliberately has no argv, write-back command,
/// default answer, or human-decision field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingHuman {
    pub schema: String,
    pub status: String,
    pub binding: GateBinding,
    pub question: String,
    pub recommendation: String,
    pub options: Vec<HumanOption>,
    pub resume_token: String,
    pub human_terminal_required: bool,
}

/// The exact host-mediated answer supplied only at resume time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeAnswer {
    pub choice_id: String,
    pub human_decision: String,
    pub free_form: Option<String>,
}

/// Journal-safe authority evidence returned after a successful one-shot
/// claim. It contains the token digest, never the raw resume token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthorityReceipt {
    pub schema: String,
    pub token_digest: String,
    pub binding: GateBinding,
    pub choice_id: String,
    pub human_decision: HumanDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_form: Option<String>,
    pub authority: String,
    pub executor: String,
}

/// Confined paths owned by one continuation. Runtime state and the isolated
/// workspace always remain below the capsule directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationPaths {
    pub directory: PathBuf,
    pub capsule: PathBuf,
    pub workspace: PathBuf,
    pub runtime_state: PathBuf,
}

/// Result of issuing a pending gate. The caller must materialize
/// `paths.workspace` and `paths.runtime_state` before returning `pending` to
/// the host; both paths are reserved but absent when this value is returned.
#[derive(Debug)]
pub struct IssuedGate {
    pub pending: PendingHuman,
    pub paths: ContinuationPaths,
}

/// A successfully claimed one-shot continuation. Its directory has already
/// moved atomically from `pending` to `claimed`, so the same token cannot win a
/// second time.
#[derive(Debug)]
pub struct ClaimedHumanDecision {
    pub receipt: AuthorityReceipt,
    pub paths: ContinuationPaths,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinuationPolicy {
    workspace: String,
    runtime_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinuationCapsule {
    schema: String,
    token_digest: String,
    binding: GateBinding,
    prompt: HumanPrompt,
    continuation: ContinuationPolicy,
}

/// Filesystem adapter for one-shot continuation capsules.
///
/// `runtime_temp_root` is trusted runtime infrastructure, not a repository
/// graph. All user-controlled token text is validated and then replaced by a
/// fixed-width digest before it participates in a path.
#[derive(Debug, Clone)]
pub struct CapsuleStore {
    root: PathBuf,
    pending: PathBuf,
    claimed: PathBuf,
}

impl CapsuleStore {
    pub fn new(runtime_temp_root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(runtime_temp_root.as_ref()).with_context(|| {
            format!(
                "creating Journey runtime temp root {}",
                runtime_temp_root.as_ref().display()
            )
        })?;
        let runtime_temp_root = runtime_temp_root
            .as_ref()
            .canonicalize()
            .context("canonicalizing Journey runtime temp root")?;
        let root = runtime_temp_root.join(STORE_DIR);
        let pending = root.join(PENDING_DIR);
        let claimed = root.join(CLAIMED_DIR);
        fs::create_dir_all(&pending)?;
        fs::create_dir_all(&claimed)?;
        let canonical_root = root.canonicalize()?;
        if !canonical_root.starts_with(&runtime_temp_root) {
            bail!("Journey gate capsule root escapes the runtime temp root");
        }
        let canonical_pending = pending.canonicalize()?;
        let canonical_claimed = claimed.canonicalize()?;
        if !canonical_pending.starts_with(&canonical_root)
            || !canonical_claimed.starts_with(&canonical_root)
        {
            bail!("Journey gate capsule directories escape their confined root");
        }
        Ok(Self {
            root: canonical_root,
            pending: canonical_pending,
            claimed: canonical_claimed,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Issue one opaque token and persist only its digest plus the exact binding
    /// and normalized prompt. This touches only the supplied runtime temp root.
    pub fn issue(&self, binding: GateBinding, prompt: HumanPrompt) -> Result<IssuedGate> {
        let prompt = prompt.normalized()?;
        binding.validate()?;
        let prompt_hash = prompt.digest()?;
        if binding.prompt_hash != prompt_hash {
            bail!(
                "gate binding prompt hash is stale (expected {}, observed {})",
                binding.prompt_hash,
                prompt_hash
            );
        }

        self.validate_store_roots()?;
        // Collection is opportunistic: a stale damaged child or concurrently
        // changing entry must never prevent issuance of a new continuation.
        let _ = self.collect_stale(SystemTime::now());
        self.validate_store_roots()?;

        for _ in 0..16 {
            let token = generate_token()?;
            let token_digest = digest_token(&token)?;
            let directory = self.pending.join(&token_digest);
            match fs::create_dir(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
            let paths = paths_for(directory);
            let capsule = ContinuationCapsule {
                schema: CONTINUATION_CAPSULE_SCHEMA.into(),
                token_digest: token_digest.clone(),
                binding: binding.clone(),
                prompt: prompt.clone(),
                continuation: ContinuationPolicy {
                    workspace: WORKSPACE_DIR.into(),
                    runtime_state: RUNTIME_STATE_FILE.into(),
                },
            };
            let installed = write_new_json(&paths.capsule, &capsule);
            if let Err(error) = installed {
                let _ = fs::remove_dir_all(&paths.directory);
                return Err(error);
            }
            return Ok(IssuedGate {
                pending: PendingHuman {
                    schema: PENDING_HUMAN_SCHEMA.into(),
                    status: "pending_human".into(),
                    binding,
                    question: prompt.question,
                    recommendation: prompt.recommendation,
                    options: prompt.options,
                    resume_token: token,
                    human_terminal_required: false,
                },
                paths,
            });
        }
        bail!("could not allocate a unique Journey gate continuation")
    }

    fn collect_stale(&self, now: SystemTime) -> Result<()> {
        for parent in [&self.pending, &self.claimed] {
            for entry in fs::read_dir(parent)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.len() != 64 || !name.bytes().all(is_lower_hex) {
                    continue;
                }
                let metadata = fs::symlink_metadata(entry.path())?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    continue;
                }
                let Ok(age) = now.duration_since(metadata.modified()?) else {
                    continue;
                };
                if age > CONTINUATION_RETENTION {
                    fs::remove_dir_all(entry.path())?;
                }
            }
        }
        Ok(())
    }

    fn validate_store_roots(&self) -> Result<()> {
        for (label, path) in [
            ("root", &self.root),
            ("pending", &self.pending),
            ("claimed", &self.claimed),
        ] {
            let metadata = fs::symlink_metadata(path)
                .with_context(|| format!("reading Journey gate {label} directory"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("Journey gate {label} path is not a confined directory");
            }
            let canonical = path
                .canonicalize()
                .with_context(|| format!("canonicalizing Journey gate {label} directory"))?;
            if canonical != *path || !canonical.starts_with(&self.root) {
                bail!("Journey gate {label} directory escapes its confined root");
            }
        }
        Ok(())
    }

    /// Validate and atomically claim a pending continuation. Invalid answers
    /// and stale bindings return before the rename, so the caller may correct
    /// the answer and retry the same current token. A valid claim is one-shot.
    pub fn claim(
        &self,
        token: &str,
        current_binding: &GateBinding,
        answer: ResumeAnswer,
        executor: &str,
    ) -> Result<ClaimedHumanDecision> {
        let token_digest = digest_token(token)?;
        let pending_dir = self.pending.join(&token_digest);
        let claimed_dir = self.claimed.join(&token_digest);
        let capsule = match read_capsule(&pending_dir) {
            Ok(capsule) => capsule,
            Err(_error) if claimed_dir.exists() => {
                bail!("Journey gate resume token has already been consumed")
            }
            Err(error) if !pending_dir.exists() => {
                return Err(error.context("unknown or unavailable Journey gate resume token"));
            }
            Err(error) => return Err(error),
        };
        validate_capsule(&capsule, &token_digest)?;
        current_binding.validate()?;
        if &capsule.binding != current_binding {
            bail!("Journey gate resume token is stale for the current binding");
        }
        let (choice_id, human_decision, free_form) = validate_answer(&capsule.prompt, answer)?;
        let executor = normalize_text("Journey gate executor", executor)?;
        let receipt = AuthorityReceipt {
            schema: AUTHORITY_RECEIPT_SCHEMA.into(),
            token_digest,
            binding: capsule.binding,
            choice_id,
            human_decision,
            free_form,
            authority: "human".into(),
            executor,
        };

        match fs::rename(&pending_dir, &claimed_dir) {
            Ok(()) => Ok(ClaimedHumanDecision {
                receipt,
                paths: paths_for(claimed_dir),
            }),
            Err(_error) if claimed_dir.exists() || !pending_dir.exists() => {
                bail!("Journey gate resume token has already been consumed")
            }
            Err(error) => Err(error).context("atomically claiming Journey gate continuation"),
        }
    }
}

/// Stable digest safe to persist or journal instead of a raw opaque token.
pub fn digest_token(token: &str) -> Result<String> {
    validate_token(token)?;
    Ok(sha256_hex(token.as_bytes()))
}

fn validate_capsule(capsule: &ContinuationCapsule, token_digest: &str) -> Result<()> {
    if capsule.schema != CONTINUATION_CAPSULE_SCHEMA {
        bail!("unsupported Journey gate continuation schema");
    }
    if capsule.token_digest != token_digest {
        bail!("Journey gate continuation token digest does not match its path");
    }
    capsule.binding.validate()?;
    if capsule.prompt.digest()? != capsule.binding.prompt_hash {
        bail!("Journey gate continuation prompt no longer matches its binding");
    }
    if capsule.continuation.workspace != WORKSPACE_DIR
        || capsule.continuation.runtime_state != RUNTIME_STATE_FILE
    {
        bail!("Journey gate continuation contains an unsafe path policy");
    }
    Ok(())
}

fn validate_answer(
    prompt: &HumanPrompt,
    answer: ResumeAnswer,
) -> Result<(String, HumanDecision, Option<String>)> {
    let option = prompt
        .options
        .iter()
        .find(|option| option.id == answer.choice_id)
        .ok_or_else(|| {
            anyhow!(
                "human choice '{}' was not offered by this gate",
                answer.choice_id
            )
        })?;
    let decision = HumanDecision::mediated(answer.human_decision)?;
    let free_form = match (option.free_form, answer.free_form) {
        (true, Some(value)) => Some(normalize_text("human free-form revision", &value)?),
        (true, None) => bail!(
            "human choice '{}' requires a substantive free-form revision",
            option.id
        ),
        (false, Some(_)) => bail!(
            "human choice '{}' does not accept a free-form revision",
            option.id
        ),
        (false, None) => None,
    };
    Ok((option.id.clone(), decision, free_form))
}

fn paths_for(directory: PathBuf) -> ContinuationPaths {
    ContinuationPaths {
        capsule: directory.join(CAPSULE_FILE),
        workspace: directory.join(WORKSPACE_DIR),
        runtime_state: directory.join(RUNTIME_STATE_FILE),
        directory,
    }
}

fn read_capsule(directory: &Path) -> Result<ContinuationCapsule> {
    let directory_metadata = fs::symlink_metadata(directory)
        .with_context(|| format!("opening Journey gate continuation {}", directory.display()))?;
    if !directory_metadata.is_dir() || directory_metadata.file_type().is_symlink() {
        bail!("Journey gate continuation path is not a confined directory");
    }
    let capsule_path = directory.join(CAPSULE_FILE);
    let capsule_metadata = fs::symlink_metadata(&capsule_path)
        .with_context(|| format!("opening Journey gate capsule {}", capsule_path.display()))?;
    if !capsule_metadata.is_file() || capsule_metadata.file_type().is_symlink() {
        bail!("Journey gate capsule path is not a regular file");
    }
    let bytes = fs::read(&capsule_path)?;
    serde_json::from_slice(&bytes).context("decoding Journey gate continuation capsule")
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn generate_token() -> Result<String> {
    let connection = rusqlite::Connection::open_in_memory()?;
    let random: String =
        connection.query_row("SELECT lower(hex(randomblob(32)))", [], |row| row.get(0))?;
    if random.len() != TOKEN_RANDOM_HEX_LEN || !random.bytes().all(is_lower_hex) {
        bail!("runtime random source returned an invalid Journey gate token");
    }
    Ok(format!("{TOKEN_PREFIX}{random}"))
}

fn validate_token(token: &str) -> Result<()> {
    let Some(random) = token.strip_prefix(TOKEN_PREFIX) else {
        bail!("invalid Journey gate resume token");
    };
    if random.len() != TOKEN_RANDOM_HEX_LEN || !random.bytes().all(is_lower_hex) {
        bail!("invalid Journey gate resume token");
    }
    Ok(())
}

fn normalize_text(label: &str, value: &str) -> Result<String> {
    let normalized = value.trim();
    validate_text(label, normalized)?;
    Ok(normalized.to_string())
}

fn validate_text(label: &str, value: &str) -> Result<()> {
    if crate::model::is_placeholder(value) {
        bail!("{label} must be substantive");
    }
    Ok(())
}

fn validate_hash(label: &str, value: &str) -> Result<()> {
    if value.len() < 8 || value.len() > 128 || !value.bytes().all(is_lower_hex) {
        bail!("{label} must be a lowercase hexadecimal digest");
    }
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(is_lower_hex) {
        bail!("{label} must be a SHA-256 hexadecimal digest");
    }
    Ok(())
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = sha256(input);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

/// Strong canonical binding helper for other runtime-owned human-authority
/// capsules. Keeping the digest implementation here ensures every
/// host-mediated seam uses the same SHA-256 primitive as Journey gates.
pub fn sha256_digest(input: &[u8]) -> String {
    sha256_hex(input)
}

// Compact SHA-256 implementation kept here so the token policy does not widen
// the crate's dependency surface. It follows FIPS 180-4 over 512-bit blocks.
fn sha256(input: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const ROUND: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().expect("four-byte SHA word"));
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let big1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(big1)
                .wrapping_add(choose)
                .wrapping_add(ROUND[index])
                .wrapping_add(words[index]);
            let big0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = big0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut out = [0_u8; 32];
    for (chunk, value) in out.chunks_exact_mut(4).zip(state) {
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    out
}
