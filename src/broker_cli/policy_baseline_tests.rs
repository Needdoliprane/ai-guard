use super::BrokerArgs;
use clap::Parser;

#[derive(Parser)]
struct BaselineCli {
    #[command(flatten)]
    _args: BrokerArgs,
}

#[test]
fn broker_policy_status_is_parsed_without_opening_an_operation() {
    let existing = BaselineCli::try_parse_from([
        "broker",
        "control-info",
        "--gateway",
        "http://127.0.0.1:1",
        "--token-file",
        "/synthetic/a1-unopened-runner-token",
    ]);
    assert!(
        existing.is_ok(),
        "setup: existing control-info must parse without opening its token file"
    );

    let parsed = BaselineCli::try_parse_from([
        "broker",
        "policy",
        "status",
        "--operation-dir",
        "/synthetic/a1-unopened-operation",
    ]);
    println!("A1_CLI_SETUP_REAL_BROKER_ARGS_CONTROL_INFO_PARSED");
    assert!(
        parsed.is_ok(),
        "A1_CLI_POLICY_STATUS_MUST_PARSE error_kind={:?}",
        parsed.as_ref().err().map(|error| error.kind())
    );
}
