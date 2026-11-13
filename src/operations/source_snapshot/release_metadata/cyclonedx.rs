//! Inventory of the locked components as a CycloneDX 1.6 JSON document,
//! lifecycle `pre-build`.
//!
//! Deterministic by construction: typed fields in a fixed order, no
//! `serialNumber` (a fixed value would contradict the uniqueness the
//! specification asks of it), no timestamp, components sorted by `bom-ref`,
//! dependencies sorted by `ref`. Only what the locks and recipes state is
//! written: Rust crates have no scope and no licence (the lock resolves
//! neither), base images and provider clients have no dependency entry (their
//! content is not observed), and no edge joins the separate graphs. No `purl`
//! or CPE is written in format 1, so the document is not ready for
//! vulnerability scanners.
use super::{
    Derived,
    declared::Known,
    document::{self, DeclaredInputs, FORMAT_VERSION, INPUTS_IDENTITY_LAYOUT},
};
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::BTreeSet;

const MAX_LICENCE_DEPTH: usize = 16;
const MAX_LICENCE_TOKENS: usize = 128;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bom {
    bom_format: &'static str,
    spec_version: &'static str,
    version: u32,
    metadata: Metadata,
    components: Vec<Component>,
    dependencies: Vec<Dependency>,
    compositions: Vec<Composition>,
}

#[derive(Serialize)]
struct Metadata {
    lifecycles: Vec<Lifecycle>,
    tools: Tools,
    component: Component,
    properties: Vec<Property>,
}

#[derive(Serialize)]
struct Lifecycle {
    phase: &'static str,
}

#[derive(Serialize)]
struct Tools {
    components: Vec<Component>,
}

#[derive(Serialize)]
struct Component {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "bom-ref", skip_serializing_if = "Option::is_none")]
    bom_ref: Option<String>,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hashes: Vec<Hash>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    licenses: Vec<LicenseChoice>,
    #[serde(rename = "externalReferences", skip_serializing_if = "Vec::is_empty")]
    external_references: Vec<ExternalReference>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    properties: Vec<Property>,
}

impl Component {
    fn new(
        kind: &'static str,
        bom_ref: Option<String>,
        name: String,
        version: Option<String>,
    ) -> Self {
        Self {
            kind,
            bom_ref,
            name,
            version,
            scope: None,
            hashes: Vec::new(),
            licenses: Vec::new(),
            external_references: Vec::new(),
            properties: Vec::new(),
        }
    }
}

#[derive(Serialize)]
struct Hash {
    alg: &'static str,
    content: String,
}

/// A licence as declared: the SPDX `expression` form only for a well-formed
/// expression, otherwise a named licence. SPDX list membership is not checked.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub(super) enum LicenseChoice {
    Expression {
        expression: String,
        acknowledgement: &'static str,
    },
    Named {
        license: NamedLicense,
    },
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(super) struct NamedLicense {
    name: String,
    acknowledgement: &'static str,
}

#[derive(Serialize)]
struct ExternalReference {
    #[serde(rename = "type")]
    kind: &'static str,
    url: String,
}

#[derive(Serialize)]
struct Property {
    name: &'static str,
    value: String,
}

#[derive(Serialize)]
struct Dependency {
    #[serde(rename = "ref")]
    reference: String,
    #[serde(rename = "dependsOn")]
    depends_on: Vec<String>,
}

#[derive(Serialize)]
struct Composition {
    aggregate: &'static str,
    assemblies: Vec<String>,
}

fn property(name: &'static str, value: impl Into<String>) -> Property {
    Property {
        name,
        value: value.into(),
    }
}

fn sorted(references: impl Iterator<Item = String>) -> Vec<String> {
    references
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// `[A-Za-z0-9.-]+` with an optional trailing `+`, and not an operator.
fn licence_identifier(word: &str) -> bool {
    let base = word.strip_suffix('+').unwrap_or(word);
    !base.is_empty()
        && !matches!(word, "AND" | "OR" | "WITH")
        && base
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
}

fn licence_tokens(text: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    for character in text.chars() {
        if character == '(' || character == ')' || character == ' ' {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            if character != ' ' {
                tokens.push(character.to_string());
            }
        } else if character.is_whitespace() || character.is_control() {
            return None;
        } else {
            word.push(character);
        }
        if tokens.len() > MAX_LICENCE_TOKENS {
            return None;
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    Some(tokens)
}

fn licence_expression(tokens: &[String], position: usize, depth: usize) -> Option<usize> {
    let mut position = licence_term(tokens, position, depth)?;
    while matches!(tokens.get(position).map(String::as_str), Some("AND" | "OR")) {
        position = licence_term(tokens, position + 1, depth)?;
    }
    Some(position)
}

fn licence_term(tokens: &[String], position: usize, depth: usize) -> Option<usize> {
    match tokens.get(position).map(String::as_str) {
        Some("(") if depth < MAX_LICENCE_DEPTH => {
            let after = licence_expression(tokens, position + 1, depth + 1)?;
            (tokens.get(after).map(String::as_str) == Some(")")).then_some(after + 1)
        }
        Some(word) if licence_identifier(word) => {
            let after = position + 1;
            match tokens.get(after).map(String::as_str) {
                Some("WITH") => tokens
                    .get(after + 1)
                    .filter(|exception| licence_identifier(exception))
                    .map(|_| after + 2),
                _ => Some(after),
            }
        }
        _ => None,
    }
}

/// One identifier, or identifiers joined by ` AND `, ` OR ` and ` WITH ` with
/// balanced parentheses.
pub(super) fn well_formed_licence(text: &str) -> bool {
    match licence_tokens(text) {
        Some(tokens) if !tokens.is_empty() => {
            licence_expression(&tokens, 0, 0) == Some(tokens.len())
        }
        _ => false,
    }
}

pub(super) fn licence(text: &str) -> LicenseChoice {
    if well_formed_licence(text) {
        LicenseChoice::Expression {
            expression: text.to_owned(),
            acknowledgement: "declared",
        }
    } else {
        LicenseChoice::Named {
            license: NamedLicense {
                name: text.to_owned(),
                acknowledgement: "declared",
            },
        }
    }
}

fn lowercase_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `(name, tag, digest)` of an image reference; the tag is what follows the
/// last `:` of the last path component, the digest what follows `@sha256:`.
fn split_reference(reference: &str) -> (String, Option<String>, Option<String>) {
    let (named, digest) = match reference.split_once("@sha256:") {
        Some((named, digest)) => (named, Some(digest.to_owned())),
        None => (reference, None),
    };
    let start = named.rfind('/').map_or(0, |slash| slash + 1);
    match named[start..].rfind(':') {
        Some(colon) => (
            named[..start + colon].to_owned(),
            Some(named[start + colon + 1..].to_owned()),
            digest,
        ),
        None => (named.to_owned(), None, digest),
    }
}

/// The SBOM of `derived` with these declared provider versions.
pub(super) fn sbom(
    derived: &Derived,
    declared: &DeclaredInputs,
    inputs_identity: &str,
) -> Result<Vec<u8>> {
    let cargo = &derived.cargo;
    let root = &cargo.packages[cargo.root];
    let root_ref = root.name.clone();
    let crate_ref = |index: usize| -> String {
        if index == cargo.root {
            root_ref.clone()
        } else {
            let package = &cargo.packages[index];
            format!("cargo:{}@{}", package.name, package.version)
        }
    };
    let mut components = Vec::new();
    let mut dependencies = Vec::new();
    for (index, package) in cargo.packages.iter().enumerate() {
        dependencies.push(Dependency {
            reference: crate_ref(index),
            depends_on: sorted(
                package
                    .dependencies
                    .iter()
                    .map(|dependency| crate_ref(*dependency)),
            ),
        });
        if index == cargo.root {
            continue;
        }
        let mut component = Component::new(
            "library",
            Some(crate_ref(index)),
            package.name.clone(),
            Some(package.version.clone()),
        );
        component.hashes = package
            .checksum
            .iter()
            .map(|checksum| Hash {
                alg: "SHA-256",
                content: checksum.clone(),
            })
            .collect();
        component.properties = vec![
            property("ai-guard:scope", "unresolved"),
            property("ai-guard:cargo-source", "crates.io"),
        ];
        components.push(component);
    }

    let npm = &derived.npm;
    let npm_ref = |index: usize| -> String {
        let package = &npm.packages[index];
        format!("npm:{}@{}", package.name, package.version)
    };
    let front_ref = format!("npm:{}@{}", npm.root.name, npm.root.version);
    let mut front = Component::new(
        "application",
        Some(front_ref.clone()),
        npm.root.name.clone(),
        Some(npm.root.version.clone()),
    );
    front.properties = vec![property("ai-guard:first-party", "true")];
    components.push(front);
    dependencies.push(Dependency {
        reference: front_ref,
        depends_on: sorted(
            npm.root
                .dependencies
                .iter()
                .map(|dependency| npm_ref(*dependency)),
        ),
    });
    for (index, package) in npm.packages.iter().enumerate() {
        let mut component = Component::new(
            "library",
            Some(npm_ref(index)),
            package.name.clone(),
            Some(package.version.clone()),
        );
        component.scope = package.dev.then_some("excluded");
        component.hashes = vec![Hash {
            alg: "SHA-512",
            content: package.sha512_hex.clone(),
        }];
        component.licenses = package
            .license
            .as_deref()
            .map(licence)
            .into_iter()
            .collect();
        component.external_references = vec![ExternalReference {
            kind: "distribution",
            url: package.resolved.clone(),
        }];
        if package.optional {
            component
                .properties
                .push(property("ai-guard:npm-optional", "true"));
        }
        for (name, values) in [
            ("ai-guard:npm-os", &package.os),
            ("ai-guard:npm-cpu", &package.cpu),
            ("ai-guard:npm-libc", &package.libc),
        ] {
            if !values.is_empty() {
                component.properties.push(property(name, values.join(",")));
            }
        }
        if package.install_script {
            component
                .properties
                .push(property("ai-guard:npm-install-script", "true"));
        }
        components.push(component);
        dependencies.push(Dependency {
            reference: npm_ref(index),
            depends_on: sorted(
                package
                    .dependencies
                    .iter()
                    .map(|dependency| npm_ref(*dependency)),
            ),
        });
    }

    let (references, _) = document::base_images(&derived.declared.recipes);
    let mut images = Vec::new();
    for reference in &references {
        let (name, tag, digest) = split_reference(reference);
        let (pinning, evidence) = match (&digest, &tag) {
            (Some(_), _) => ("digest", "declared:digest"),
            (None, Some(_)) => ("tag", "declared:tag"),
            (None, None) => ("none", "declared:none"),
        };
        let bom_ref = format!("image:{reference}");
        let mut component = Component::new("container", Some(bom_ref.clone()), name, tag);
        component.hashes = digest
            .filter(|digest| lowercase_hex64(digest))
            .map(|content| Hash {
                alg: "SHA-256",
                content,
            })
            .into_iter()
            .collect();
        component.properties = vec![
            property("ai-guard:pinning", pinning),
            property("ai-guard:version-evidence", evidence),
        ];
        for recipe in &derived.declared.recipes {
            let declares = recipe.base_images.value().is_some_and(|images| {
                images.iter().any(|image| {
                    image
                        .value()
                        .is_some_and(|image| !image.internal_stage && image.reference == *reference)
                })
            });
            if declares {
                component
                    .properties
                    .push(property("ai-guard:declared-by", recipe.path.clone()));
            }
        }
        components.push(component);
        images.push(bom_ref);
    }

    if let Known::Declared { value, .. } = &derived.declared.toolchain {
        let mut component = Component::new(
            "application",
            Some(format!("toolchain:rust@{}", value.channel)),
            "rust".to_owned(),
            Some(value.channel.clone()),
        );
        component.properties = vec![property(
            "ai-guard:version-evidence",
            "declared:rust-toolchain.toml",
        )];
        components.push(component);
    }

    let mut providers = Vec::new();
    for (version, package) in [
        (&declared.claude_code, &derived.declared.claude_code_package),
        (&declared.codex, &derived.declared.codex_package),
    ] {
        if let (Some(version), Some(package)) = (version, package.value()) {
            let bom_ref = format!("provider:{package}@{version}");
            let mut component = Component::new(
                "application",
                Some(bom_ref.clone()),
                package.clone(),
                Some(version.clone()),
            );
            component.properties = vec![property(
                "ai-guard:version-evidence",
                "declared:build-argument",
            )];
            components.push(component);
            providers.push(bom_ref);
        }
    }

    components.sort_by(|left, right| left.bom_ref.cmp(&right.bom_ref));
    let mut seen = BTreeSet::new();
    for component in &components {
        if !seen.insert(component.bom_ref.clone()) {
            bail!(
                "two inventory components would share the reference {}; the inventory is not \
                 written",
                component.bom_ref.as_deref().unwrap_or_default()
            );
        }
    }
    if seen.contains(&Some(root_ref.clone())) {
        bail!("an inventory component would share the reference of the described package");
    }
    dependencies.sort_by(|left, right| left.reference.cmp(&right.reference));

    let mut metadata_component = Component::new(
        "application",
        Some(root_ref.clone()),
        root.name.clone(),
        Some(root.version.clone()),
    );
    if let Some(license) = derived.declared.package_license.value() {
        metadata_component.licenses = vec![licence(license)];
    }
    let mut compositions = vec![Composition {
        aggregate: "incomplete",
        assemblies: vec![root_ref],
    }];
    if !images.is_empty() {
        compositions.push(Composition {
            aggregate: "unknown",
            assemblies: images,
        });
    }
    if !providers.is_empty() {
        compositions.push(Composition {
            aggregate: "unknown",
            assemblies: providers,
        });
    }
    let bom = Bom {
        bom_format: "CycloneDX",
        spec_version: "1.6",
        version: 1,
        metadata: Metadata {
            lifecycles: vec![Lifecycle { phase: "pre-build" }],
            tools: Tools {
                components: vec![Component::new(
                    "application",
                    None,
                    env!("CARGO_PKG_NAME").to_owned(),
                    Some(env!("CARGO_PKG_VERSION").to_owned()),
                )],
            },
            component: metadata_component,
            properties: vec![
                property(
                    "ai-guard:source-digest",
                    derived.source.digest.value.clone(),
                ),
                property(
                    "ai-guard:source-digest-layout",
                    derived.source.digest.layout.clone(),
                ),
                property("ai-guard:build-inputs-identity", inputs_identity),
                property(
                    "ai-guard:build-inputs-identity-layout",
                    INPUTS_IDENTITY_LAYOUT,
                ),
                property(
                    "ai-guard:release-manifest-format",
                    FORMAT_VERSION.to_string(),
                ),
                property("ai-guard:hash-subject", "registry-archive"),
                property(
                    "ai-guard:dependency-graph",
                    "lock graphs only; image assembly not represented",
                ),
                property("ai-guard:package-identifiers", "none"),
            ],
        },
        components,
        dependencies,
        compositions,
    };
    document::canonical(&bom)
}
