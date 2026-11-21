//! One list of human capability names: the Rust enum, its CLI flags, the
//! dashboard parser and the SSO guides must name exactly the same set, in the
//! same order. A new variant fails to compile until `ALL` and this test know it.
use agentsandbox::{identity::HumanCapabilityV1, identity_cli::Capability};
use clap::ValueEnum;

fn position(capability: HumanCapabilityV1) -> usize {
    use HumanCapabilityV1 as C;
    match capability {
        C::ReadMetadata => 0,
        C::ReadContent => 1,
        C::Export => 2,
        C::ReadTenantMetadata => 3,
        C::ManageMemberships => 4,
        C::ManageGrants => 5,
        C::ReadBrokerRegistry => 6,
        C::ReadWorkerRegistry => 7,
        C::ApproveLaunch => 8,
        C::ReadMemberships => 9,
        C::ReadConfiguration => 10,
        C::LaunchSession => 11,
        C::SendSessionMessage => 12,
        C::ApproveEgress => 13,
    }
}
fn rust_names() -> Vec<String> {
    HumanCapabilityV1::ALL
        .iter()
        .enumerate()
        .map(|(index, capability)| {
            assert_eq!(position(*capability), index, "ALL keeps the wire order");
            serde_json::to_value(capability)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}
fn repository_file(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|error| panic!("read {path}: {error}"))
}
/// Every backquoted word of `text` from `start` up to the next full stop.
fn names_after(text: &str, start: &str) -> Vec<String> {
    let begin = text
        .find(start)
        .unwrap_or_else(|| panic!("missing sentence {start:?}"));
    let sentence = &text[begin..];
    let sentence = &sentence[..sentence.find(". ").unwrap_or(sentence.len())];
    sentence
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_list_of_capability_names_matches_the_rust_enum() {
    let rust = rust_names();
    assert_eq!(rust.len(), HumanCapabilityV1::ALL.len());

    let cli: Vec<String> = Capability::value_variants()
        .iter()
        .map(|flag| {
            let name = flag.to_possible_value().unwrap().get_name().to_owned();
            let mapped = serde_json::to_value(HumanCapabilityV1::from(*flag)).unwrap();
            assert_eq!(name.replace('-', "_"), mapped.as_str().unwrap());
            mapped.as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(cli, rust, "identity CLI flags");

    let front = repository_file("front/src/human-model.ts");
    let start = front
        .find("export const CAPABILITIES = [")
        .expect("the dashboard keeps one capability list");
    let list = &front[start..start + front[start..].find("] as const").unwrap()];
    let front: Vec<String> = list
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    assert_eq!(front, rust, "front/src/human-model.ts CAPABILITIES");

    for (path, sentence) in [
        (
            "docs/setup/sso/README.md",
            "The accepted capability names are",
        ),
        (
            "docs/setup/sso/README.fr.md",
            "Les capacités acceptées sont",
        ),
    ] {
        assert_eq!(
            names_after(&repository_file(path), sentence),
            rust,
            "{path} accepted names"
        );
    }
}
