//! Facts the build inputs declare: the package, its licence and release
//! profile (`Cargo.toml`), the requested toolchain (`rust-toolchain.toml`)
//! and, per recipe, its base images and `cargo build` invocations.
//!
//! These files are read leniently: what format 1 cannot interpret stays
//! [`Known::Unknown`] with its reason, never a default. A recipe is Dockerfile
//! text read without any shell interpretation; a declared profile or target is
//! what the tokens of an invocation say, not what a build did.
use super::toml_subset::{self, Item, Value};
use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeMap};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) const CARGO_TOML: &str = "Cargo.toml";
pub(super) const TOOLCHAIN: &str = "rust-toolchain.toml";
pub(super) const WORKER_RECIPE: &str = "images/worker.Dockerfile";
/// The pattern the worker recipe applies to `CODEX_VERSION` and
/// `CLAUDE_CODE_VERSION` before installing a provider client
/// (`images/worker.Dockerfile`, both `grep -Eq` lines). A unit test finds it
/// there twice, so a change of the recipe fails a test instead of diverging.
pub(super) const RECIPE_VERSION_PATTERN: &str = r"^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$";
pub(super) const CODEX_PACKAGE: &str = "@openai/codex";
pub(super) const CLAUDE_CODE_PACKAGE: &str = "@anthropic-ai/claude-code";
const CODEX_PACKAGE_TOKEN: &str = "\"@openai/codex@$CODEX_VERSION\"";
const CLAUDE_CODE_PACKAGE_TOKEN: &str = "\"@anthropic-ai/claude-code@$CLAUDE_CODE_VERSION\"";
const NO_TARGET: &str = "no --target; the builder platform is not observed";

/// A fact a build input declares, or why it is not known. Serialised as
/// `{"status": "declared", "value": …, "from": …}` or `{"status": "unknown",
/// "reason": …}`; any other combination, or another key, is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Known<T> {
    Declared { value: T, from: String },
    Unknown { reason: String },
}

impl<T> Known<T> {
    pub(super) fn declared(value: T, from: &str) -> Self {
        Self::Declared {
            value,
            from: from.to_owned(),
        }
    }

    pub(super) fn unknown(reason: impl Into<String>) -> Self {
        Self::Unknown {
            reason: reason.into(),
        }
    }

    pub(super) fn value(&self) -> Option<&T> {
        match self {
            Self::Declared { value, .. } => Some(value),
            Self::Unknown { .. } => None,
        }
    }
}

impl<T: Serialize> Serialize for Known<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Declared { value, from } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("status", "declared")?;
                map.serialize_entry("value", value)?;
                map.serialize_entry("from", from)?;
                map.end()
            }
            Self::Unknown { reason } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("status", "unknown")?;
                map.serialize_entry("reason", reason)?;
                map.end()
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnownWire<T> {
    status: String,
    value: Option<T>,
    from: Option<String>,
    reason: Option<String>,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Known<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = KnownWire::<T>::deserialize(deserializer)?;
        match (wire.status.as_str(), wire.value, wire.from, wire.reason) {
            ("declared", Some(value), Some(from), None) => Ok(Self::Declared { value, from }),
            ("unknown", None, None, Some(reason)) => Ok(Self::Unknown { reason }),
            _ => Err(serde::de::Error::custom(
                "a known value is either declared with its value and source, or unknown with its \
                 reason",
            )),
        }
    }
}

/// A scalar value of `[profile.release]`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub(super) enum ScalarV1 {
    Bool(bool),
    Integer(i64),
    String(String),
}

/// The toolchain `rust-toolchain.toml` requests, never the compiler that ran.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolchainV1 {
    pub(super) channel: String,
    pub(super) profile: Option<String>,
    pub(super) components: Vec<String>,
    pub(super) targets: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct BaseImageV1 {
    pub(super) reference: String,
    pub(super) alias: Option<String>,
    /// The reference names an earlier stage of the same recipe.
    pub(super) internal_stage: bool,
    pub(super) pinned_by_digest: bool,
}

/// One `RUN cargo build …` of a recipe, as its tokens declare it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct InvocationV1 {
    pub(super) argv: Vec<String>,
    /// `release` for `--release`/`-r`, `P` for `--profile P`, `dev` otherwise.
    pub(super) profile: String,
    /// Declared only by `--target T`.
    pub(super) target: Known<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct RecipeV1 {
    pub(super) path: String,
    pub(super) sha256: String,
    pub(super) base_images: Known<Vec<Known<BaseImageV1>>>,
    pub(super) cargo_build_invocations: Known<Vec<Known<InvocationV1>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DeclaredBuild {
    pub(super) package_license: Known<String>,
    /// `[package]` name and version.
    pub(super) cargo_package: Known<(String, String)>,
    pub(super) release_profile: Known<BTreeMap<String, ScalarV1>>,
    pub(super) toolchain: Known<ToolchainV1>,
    /// Every prepared `images/*.Dockerfile`, sorted by path.
    pub(super) recipes: Vec<RecipeV1>,
    pub(super) codex_package: Known<String>,
    pub(super) claude_code_package: Known<String>,
}

/// `recipes` are `(path, bytes)` of every prepared `images/*.Dockerfile`.
pub(super) fn read(
    cargo_toml: &[u8],
    toolchain_toml: &[u8],
    recipes: &[(&str, &[u8])],
) -> DeclaredBuild {
    let (cargo_package, package_license, release_profile) = cargo_manifest(cargo_toml);
    let worker = recipes
        .iter()
        .find(|(path, _)| *path == WORKER_RECIPE)
        .map(|(_, bytes)| *bytes);
    let mut read: Vec<RecipeV1> = recipes
        .iter()
        .map(|(path, bytes)| recipe(path, bytes))
        .collect();
    read.sort_by(|left, right| left.path.cmp(&right.path));
    DeclaredBuild {
        package_license,
        cargo_package,
        release_profile,
        toolchain: toolchain(toolchain_toml),
        recipes: read,
        codex_package: provider_package(worker, CODEX_PACKAGE_TOKEN, CODEX_PACKAGE),
        claude_code_package: provider_package(
            worker,
            CLAUDE_CODE_PACKAGE_TOKEN,
            CLAUDE_CODE_PACKAGE,
        ),
    }
}

/// Both rules: `operations::validate_provider_version` and the pattern of the
/// worker recipe, so that no manifest declares a version with which no worker
/// image can be built.
pub(super) fn validate_declared_provider_version(version: &str) -> Result<()> {
    crate::operations::validate_provider_version(version)?;
    let pattern = regex::Regex::new(RECIPE_VERSION_PATTERN)?;
    ensure!(
        pattern.is_match(version),
        "provider version {version} is refused: the worker recipe accepts only versions matching \
         {RECIPE_VERSION_PATTERN}, so no worker image can be built with it"
    );
    Ok(())
}

type CargoFacts = (
    Known<(String, String)>,
    Known<String>,
    Known<BTreeMap<String, ScalarV1>>,
);

/// How a value is written, so that a reason names its form without
/// repeating it.
fn form(value: &Value) -> &'static str {
    match value {
        Value::Table(_) => "an inline table",
        Value::Array(_) => "an array",
        Value::String(_) => "a string",
        Value::Integer(_) => "an integer",
        Value::Bool(_) => "a boolean",
    }
}

/// Keeps the first reason, in file order, why a fact is unknown.
fn first_reason<T>(fact: &mut std::result::Result<T, String>, reason: String) {
    if fact.is_ok() {
        *fact = Err(reason);
    }
}

/// `[package]` name, version and licence, and the scalar values of the
/// `[profile.release]` table. The same settings written in another legal
/// form (a root `package` or `profile` key, a `release` key under
/// `[profile]`, an array of tables, a nested table or a non-scalar value) are
/// not read: the facts they would change are unknown with the line, never
/// declared empty.
fn cargo_manifest(bytes: &[u8]) -> CargoFacts {
    let items = match toml_subset::parse(bytes) {
        Ok(items) => items,
        Err(refused) => {
            let reason = format!(
                "{CARGO_TOML} line {}: {}; format 1 reads a subset of TOML",
                refused.line, refused.reason
            );
            return (
                Known::unknown(&reason),
                Known::unknown(&reason),
                Known::unknown(reason),
            );
        }
    };
    let mut section: Option<Vec<String>> = Some(Vec::new());
    let mut package: BTreeMap<String, Value> = BTreeMap::new();
    let mut package_form: std::result::Result<(), String> = Ok(());
    let mut profile: std::result::Result<BTreeMap<String, ScalarV1>, String> = Ok(BTreeMap::new());
    for item in items {
        match item {
            Item::Header { array, path, line } => {
                let release = path.len() >= 2 && path[0] == "profile" && path[1] == "release";
                if release && path.len() > 2 {
                    first_reason(
                        &mut profile,
                        format!(
                            "{CARGO_TOML} line {line}: the nested table [{}] changes the release \
                             profile; format 1 records the scalar values of [profile.release] only",
                            path.join(".")
                        ),
                    );
                } else if array && (release || (path.len() == 1 && path[0] == "profile")) {
                    first_reason(
                        &mut profile,
                        format!(
                            "{CARGO_TOML} line {line}: [[{}]] declares the profiles as an array of \
                             tables; format 1 records the scalar values of a [profile.release] \
                             table only",
                            path.join(".")
                        ),
                    );
                }
                if array && path.len() == 1 && path[0] == "package" {
                    first_reason(
                        &mut package_form,
                        format!(
                            "{CARGO_TOML} line {line}: [[package]] declares the package as an \
                             array of tables; format 1 reads name, version and licence from a \
                             [package] table only"
                        ),
                    );
                }
                section = (!array).then_some(path);
            }
            Item::Pair { key, value, line } => match section.as_deref() {
                Some([]) if key == "package" => first_reason(
                    &mut package_form,
                    format!(
                        "{CARGO_TOML} line {line}: the root key package sets the package as {}; \
                         format 1 reads name, version and licence from a [package] table only",
                        form(&value)
                    ),
                ),
                Some([]) if key == "profile" => first_reason(
                    &mut profile,
                    format!(
                        "{CARGO_TOML} line {line}: the root key profile sets the profiles as {}; \
                         format 1 records the scalar values of a [profile.release] table only",
                        form(&value)
                    ),
                ),
                Some([table]) if table == "profile" && key == "release" => first_reason(
                    &mut profile,
                    format!(
                        "{CARGO_TOML} line {line}: [profile] sets release as {}; format 1 records \
                         the scalar values of a [profile.release] table only",
                        form(&value)
                    ),
                ),
                Some([table]) if table == "package" => {
                    if matches!(key.as_str(), "name" | "version" | "license") {
                        package.insert(key, value);
                    }
                }
                Some([table, name]) if table == "profile" && name == "release" => {
                    let scalar = match value {
                        Value::Bool(flag) => Ok(ScalarV1::Bool(flag)),
                        Value::Integer(number) => Ok(ScalarV1::Integer(number)),
                        Value::String(text) => Ok(ScalarV1::String(text)),
                        Value::Array(items) => Err(format!(
                            "{CARGO_TOML} line {line}: [profile.release] {key} is an array of {} \
                             values; format 1 records scalar values only",
                            items.len()
                        )),
                        Value::Table(entries) => Err(format!(
                            "{CARGO_TOML} line {line}: [profile.release] {key} is an inline table \
                             of {} keys; format 1 records scalar values only",
                            entries.len()
                        )),
                    };
                    match scalar {
                        Ok(scalar) => {
                            if let Ok(values) = profile.as_mut() {
                                values.insert(key, scalar);
                            }
                        }
                        Err(reason) => first_reason(&mut profile, reason),
                    }
                }
                _ => {}
            },
        }
    }
    let release_profile = match profile {
        Ok(values) => Known::declared(values, CARGO_TOML),
        Err(reason) => Known::unknown(reason),
    };
    if let Err(reason) = package_form {
        return (
            Known::unknown(&reason),
            Known::unknown(reason),
            release_profile,
        );
    }
    let string = |key: &str| match package.get(key) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    };
    let cargo_package = match (string("name"), string("version")) {
        (Some(name), Some(version)) => Known::declared((name, version), CARGO_TOML),
        _ => Known::unknown(format!(
            "{CARGO_TOML} [package] declares no name and version as plain strings"
        )),
    };
    let license = match string("license") {
        Some(license) => Known::declared(license, CARGO_TOML),
        None => Known::unknown(format!(
            "{CARGO_TOML} [package] declares no licence as a plain string"
        )),
    };
    (cargo_package, license, release_profile)
}

fn strings(values: Vec<Value>) -> Option<Vec<String>> {
    values
        .into_iter()
        .map(|value| match value {
            Value::String(text) => Some(text),
            _ => None,
        })
        .collect()
}

fn toolchain(bytes: &[u8]) -> Known<ToolchainV1> {
    let items = match toml_subset::parse(bytes) {
        Ok(items) => items,
        Err(refused) => {
            return Known::unknown(format!(
                "{TOOLCHAIN} line {}: {}; format 1 reads a subset of TOML",
                refused.line, refused.reason
            ));
        }
    };
    let mut in_table = false;
    let mut channel = None;
    let mut profile = None;
    let mut components = Vec::new();
    let mut targets = Vec::new();
    for item in items {
        match item {
            Item::Header { array, path, line } => {
                if array || in_table || path.len() != 1 || path[0] != "toolchain" {
                    return Known::unknown(format!(
                        "{TOOLCHAIN} line {line}: format 1 interprets one [toolchain] table only"
                    ));
                }
                in_table = true;
            }
            Item::Pair { key, value, line } => {
                if !in_table {
                    return Known::unknown(format!(
                        "{TOOLCHAIN} line {line}: key {key} outside [toolchain] is not \
                         interpreted"
                    ));
                }
                let list = |values: Vec<Value>| {
                    strings(values).ok_or_else(|| {
                        format!("{TOOLCHAIN} line {line}: {key} must be an array of strings")
                    })
                };
                let read = match (key.as_str(), value) {
                    ("channel", Value::String(text)) => {
                        channel = Some(text);
                        Ok(())
                    }
                    ("profile", Value::String(text)) => {
                        profile = Some(text);
                        Ok(())
                    }
                    ("components", Value::Array(values)) => {
                        list(values).map(|values| components = values)
                    }
                    ("targets", Value::Array(values)) => {
                        list(values).map(|values| targets = values)
                    }
                    _ => Err(format!(
                        "{TOOLCHAIN} line {line}: key {key} or its value is not interpreted in \
                         format 1"
                    )),
                };
                if let Err(reason) = read {
                    return Known::unknown(reason);
                }
            }
        }
    }
    match channel {
        Some(channel) => Known::declared(
            ToolchainV1 {
                channel,
                profile,
                components,
                targets,
            },
            TOOLCHAIN,
        ),
        None => Known::unknown(format!("{TOOLCHAIN} declares no channel")),
    }
}

fn provider_package(worker: Option<&[u8]>, token: &str, package: &str) -> Known<String> {
    let Some(worker) = worker else {
        return Known::unknown(format!("{WORKER_RECIPE} is not a prepared build input"));
    };
    let token = token.as_bytes();
    if worker.windows(token.len()).any(|window| window == token) {
        Known::declared(package.to_owned(), WORKER_RECIPE)
    } else {
        Known::unknown(format!(
            "{WORKER_RECIPE} does not hold the exact install token of {package}"
        ))
    }
}

/// The base images and `cargo build` invocations of one recipe.
pub(super) fn recipe(path: &str, bytes: &[u8]) -> RecipeV1 {
    let sha256 = hex::encode(Sha256::digest(bytes));
    let (base_images, cargo_build_invocations) = match instructions(bytes) {
        Ok(instructions) => (
            base_images(path, &instructions),
            invocations(path, &instructions),
        ),
        Err(reason) => {
            let reason = format!("{path} {reason}");
            (Known::unknown(&reason), Known::unknown(reason))
        }
    };
    RecipeV1 {
        path: path.to_owned(),
        sha256,
        base_images,
        cargo_build_invocations,
    }
}

/// `(text without its continuation mark, continues)`. The mark is a `\` that
/// ends the line, blanks after it allowed.
fn continuation(line: &str) -> (&str, bool) {
    match line.trim_end().strip_suffix('\\') {
        Some(body) => (body, true),
        None => (line, false),
    }
}

/// A comment of the form `# <word>=<value>`, which before the first
/// instruction is a parser directive (`syntax`, `escape`, `check`, …).
fn directive(comment: &str) -> bool {
    let body = comment.trim_start_matches('#').trim_start();
    let word = body
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
        .count();
    word > 0 && body[word..].trim_start().starts_with('=')
}

/// Logical instructions `(first line, joined text)`. A parser directive, a
/// heredoc, or a comment or blank line inside a continued instruction makes
/// the whole recipe unknown: format 1 does not reproduce the rules of the
/// Dockerfile frontend for them, and no instruction is split or dropped.
fn instructions(bytes: &[u8]) -> std::result::Result<Vec<(usize, String)>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "is not UTF-8".to_owned())?;
    let mut instructions = Vec::new();
    let mut current: Option<(usize, String)> = None;
    let mut before_first = true;
    for (index, raw) in text.split('\n').enumerate() {
        let number = index + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = line.trim();
        let comment = trimmed.starts_with('#');
        if let Some((start, mut joined)) = current.take() {
            if trimmed.is_empty() || comment {
                return Err(format!(
                    "line {number}: a comment or a blank line inside a continued instruction is \
                     outside format 1"
                ));
            }
            let (body, continues) = continuation(line);
            joined.push_str(body);
            if continues {
                current = Some((start, joined));
            } else {
                instructions.push((start, joined));
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if comment {
            if before_first && directive(trimmed) {
                return Err(format!(
                    "line {number}: a parser directive can change how the recipe is read; it is \
                     outside format 1"
                ));
            }
            continue;
        }
        before_first = false;
        let (body, continues) = continuation(line);
        if continues {
            current = Some((number, body.to_owned()));
        } else {
            instructions.push((number, body.to_owned()));
        }
    }
    if let Some(unfinished) = current {
        instructions.push(unfinished);
    }
    if let Some((line, _)) = instructions.iter().find(|(_, text)| text.contains("<<")) {
        return Err(format!(
            "line {line}: an instruction holds <<, which can start a heredoc; heredocs are outside \
             format 1"
        ));
    }
    Ok(instructions)
}

/// The keyword of an instruction, uppercase, and the text after it.
fn split_keyword(text: &str) -> Option<(String, &str)> {
    let trimmed = text.trim_start();
    let keyword = trimmed.split_whitespace().next()?;
    Some((
        keyword.to_ascii_uppercase(),
        trimmed[keyword.len()..].trim(),
    ))
}

fn base_images(path: &str, instructions: &[(usize, String)]) -> Known<Vec<Known<BaseImageV1>>> {
    let mut images = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    for (line, text) in instructions {
        let Some((keyword, rest)) = split_keyword(text) else {
            continue;
        };
        if keyword != "FROM" {
            continue;
        }
        let tokens: Vec<&str> = rest
            .split_whitespace()
            .filter(|token| !token.starts_with("--"))
            .collect();
        let (reference, alias) = match tokens.as_slice() {
            [reference] => (*reference, None),
            [reference, word, alias] if word.eq_ignore_ascii_case("AS") => {
                (*reference, Some(*alias))
            }
            _ => {
                images.push(Known::unknown(format!(
                    "{path} line {line}: FROM is outside the form FROM [--platform=P] REF [AS NAME]"
                )));
                continue;
            }
        };
        if reference.contains('$') {
            images.push(Known::unknown(format!(
                "{path} line {line}: the FROM reference {reference} uses a build argument"
            )));
        } else {
            let internal_stage = aliases.contains(&reference.to_ascii_lowercase());
            images.push(Known::declared(
                BaseImageV1 {
                    reference: reference.to_owned(),
                    alias: alias.map(str::to_owned),
                    internal_stage,
                    pinned_by_digest: !internal_stage && reference.contains("@sha256:"),
                },
                path,
            ));
        }
        if let Some(alias) = alias {
            aliases.push(alias.to_ascii_lowercase());
        }
    }
    if images.is_empty() {
        Known::unknown(format!("{path}: no FROM instruction"))
    } else {
        Known::declared(images, path)
    }
}

/// `cargo` as a word: bounded by the start or end of the text, a blank, one
/// of `; & | ( )`, or one of `" ' [ ] ,` so that the exec form
/// `RUN ["cargo", …]` and a quoted `sh -c "cargo …"` are recognised too (and
/// then left unknown) instead of being missed.
fn mentions_cargo(text: &str) -> bool {
    let bytes = text.as_bytes();
    let boundary = |byte: Option<&u8>| match byte {
        None => true,
        Some(byte) => byte.is_ascii_whitespace() || b";&|()\"'[],".contains(byte),
    };
    text.match_indices("cargo").any(|(start, word)| {
        boundary(start.checked_sub(1).and_then(|before| bytes.get(before)))
            && boundary(bytes.get(start + word.len()))
    })
}

fn invocations(path: &str, instructions: &[(usize, String)]) -> Known<Vec<Known<InvocationV1>>> {
    let mut found = Vec::new();
    for (line, text) in instructions {
        let Some((keyword, rest)) = split_keyword(text) else {
            continue;
        };
        if keyword == "RUN" && mentions_cargo(rest) {
            found.push(invocation(path, *line, rest));
        }
    }
    Known::declared(found, path)
}

fn profile_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn target_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

/// The admitted form `RUN cargo build <tokens>`: no shell syntax in any token,
/// at most one profile option and at most one `--target`.
fn invocation(path: &str, line: usize, body: &str) -> Known<InvocationV1> {
    let unknown = |why: &str| Known::unknown(format!("{path} line {line}: {why}"));
    let tokens: Vec<&str> = body.split_whitespace().collect();
    if tokens.len() < 2 || tokens[0] != "cargo" || tokens[1] != "build" {
        return unknown(
            "a RUN instruction uses cargo outside the form RUN cargo build <arguments>; format 1 \
             does not interpret shell commands",
        );
    }
    if tokens.iter().any(|token| {
        token
            .chars()
            .any(|character| "\\;&|$`()<>\"'".contains(character))
    }) {
        return unknown("the cargo build arguments hold shell syntax");
    }
    let mut profile: Option<String> = None;
    let mut target: Option<String> = None;
    let mut index = 2;
    while index < tokens.len() {
        let token = tokens[index];
        let (option, value) = if let Some(value) = token.strip_prefix("--profile=") {
            ("profile", Some(value))
        } else if let Some(value) = token.strip_prefix("--target=") {
            ("target", Some(value))
        } else if token == "--profile" || token == "--target" {
            index += 1;
            let Some(value) = tokens.get(index) else {
                return unknown("an option has no value");
            };
            (&token[2..], Some(*value))
        } else if token == "--release" || token == "-r" {
            ("profile", None)
        } else if token.starts_with('-') && !token.starts_with("--") && token.len() > 2 {
            return unknown("combined short options are not interpreted");
        } else {
            index += 1;
            continue;
        };
        index += 1;
        match option {
            "profile" => {
                let name = value.unwrap_or("release");
                if !profile_name(name) {
                    return unknown("a profile name is outside [A-Za-z0-9_-]{1,64}");
                }
                if profile.replace(name.to_owned()).is_some() {
                    return unknown("more than one profile option");
                }
            }
            _ => {
                let name = value.unwrap_or_default();
                if !target_name(name) {
                    return unknown("a target is outside [A-Za-z0-9_.-]{1,64}");
                }
                if target.replace(name.to_owned()).is_some() {
                    return unknown("more than one --target");
                }
            }
        }
    }
    Known::declared(
        InvocationV1 {
            argv: tokens.iter().map(|token| (*token).to_owned()).collect(),
            profile: profile.unwrap_or_else(|| "dev".to_owned()),
            target: match target {
                Some(target) => Known::declared(target, path),
                None => Known::unknown(NO_TARGET),
            },
        },
        path,
    )
}
