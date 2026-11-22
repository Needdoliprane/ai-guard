//! Closed catalogue of finding codes.
//!
//! One code has one axis, one status and one basis for the whole of report
//! version 1, starting at the corrected public integration freeze. A code
//! retired after that first publication stays in the catalogue and is never reused.
//! Before publication, provider.selection_resolution_not_available and
//! identity.idp_reachability_not_available were removed; their IDs stay reserved.
//!
//! Every text here is static and must be a fixpoint of the output filter
//! (`crate::redaction`): no filter word directly before `:` or `=`, and no
//! `Bearer` or `Basic` followed by a word. A text names only commands and
//! options that exist; `commands` lists them so a test can ask each one for
//! its help. `{target}` in an action is replaced by the quoted display of the
//! operator's own input.

use super::report::{Axis, Basis, Status};
use serde::{Deserialize, Serialize};

/// Replaced in an action by the quoted display of the finding's input.
pub const TARGET_PLACEHOLDER: &str = "{target}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    /// The product CLI, `agentsandbox`.
    Product,
    /// This tool.
    Diagnose,
}

/// A command a catalogue text names, with the options the text names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandRef {
    pub program: Program,
    pub subcommand: &'static [&'static str],
    pub flags: &'static [&'static str],
}

const fn product(
    subcommand: &'static [&'static str],
    flags: &'static [&'static str],
) -> CommandRef {
    CommandRef {
        program: Program::Product,
        subcommand,
        flags,
    }
}

const fn tool(flags: &'static [&'static str]) -> CommandRef {
    CommandRef {
        program: Program::Diagnose,
        subcommand: &[],
        flags,
    }
}

/// Texts that replace the generic ones in a named situation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextVariant {
    pub name: &'static str,
    pub message: Option<&'static str>,
    pub action: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogueEntry {
    pub code: FindingCode,
    pub axis: Axis,
    pub status: Status,
    pub basis: Basis,
    /// A limit is fixed for a build and reported with the scope `build`.
    pub limit: bool,
    /// The command that applies the rule, when one does.
    pub consumer: Option<&'static str>,
    pub message: &'static str,
    pub action: &'static str,
    pub requires: Option<&'static str>,
    pub variants: &'static [TextVariant],
    pub commands: &'static [CommandRef],
}

impl CatalogueEntry {
    fn variant(&self, name: Option<&str>) -> Option<&'static TextVariant> {
        name.and_then(|name| self.variants.iter().find(|entry| entry.name == name))
    }

    /// The message of a named situation, or the generic message.
    pub fn message_for(&self, variant: Option<&str>) -> &'static str {
        self.variant(variant)
            .and_then(|entry| entry.message)
            .unwrap_or(self.message)
    }

    /// The action of a named situation, or the generic action.
    pub fn action_for(&self, variant: Option<&str>) -> &'static str {
        self.variant(variant)
            .and_then(|entry| entry.action)
            .unwrap_or(self.action)
    }

    /// Every action the entry can print.
    pub fn actions(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.action).chain(self.variants.iter().filter_map(|entry| entry.action))
    }

    /// Every static text of the entry, for tests and documents.
    pub fn texts(&self) -> impl Iterator<Item = &'static str> {
        [
            Some(self.message),
            Some(self.action),
            self.requires,
            self.consumer,
        ]
        .into_iter()
        .flatten()
        .chain(
            self.variants
                .iter()
                .flat_map(|entry| [entry.message, entry.action])
                .flatten(),
        )
    }
}

macro_rules! optional {
    () => {
        None
    };
    ($value:literal) => {
        Some($value)
    };
}

macro_rules! or_else {
    (; $default:expr) => {
        $default
    };
    ($value:expr; $default:expr) => {
        $value
    };
}

macro_rules! catalogue {
    ($(
        $variant:ident = $code:literal {
            axis: $axis:ident, status: $status:ident, basis: $basis:ident,
            $(limit: $limit:literal,)?
            $(consumer: $consumer:literal,)?
            message: $message:literal,
            action: $action:literal,
            $(requires: $requires:literal,)?
            $(variants: $variants:expr,)?
            $(commands: $commands:expr,)?
        }
    )*) => {
        /// Closed set of finding codes. The serialised form is the dotted code.
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        pub enum FindingCode {
            $(
                #[serde(rename = $code)]
                $variant,
            )*
        }

        impl FindingCode {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)*
                }
            }
        }

        /// One entry per code, in the declaration order of `FindingCode`.
        pub const CATALOGUE: &[CatalogueEntry] = &[
            $(CatalogueEntry {
                code: FindingCode::$variant,
                axis: Axis::$axis,
                status: Status::$status,
                basis: Basis::$basis,
                limit: or_else!($($limit)?; false),
                consumer: optional!($($consumer)?),
                message: $message,
                action: $action,
                requires: optional!($($requires)?),
                variants: or_else!($($variants)?; &[]),
                commands: or_else!($($commands)?; &[]),
            },)*
        ];
    };
}

impl FindingCode {
    pub fn entry(self) -> &'static CatalogueEntry {
        // The macro emits the table in the declaration order of the enum.
        &CATALOGUE[self as usize]
    }
}

catalogue! {
    // ---- layout -----------------------------------------------------------
    LayoutHistoricalConfigDir = "layout.historical_config_dir" {
        axis: Layout, status: Info, basis: Observed,
        message: "This directory has the historical layout created by agentsandbox init: private key sources, their read-only projections, sessions and pending operations.",
        action: "No action is needed. The commands init, up, down, run, worker-status, worker-resume, worker-stop, export-workspace, verify-audit and providers read this layout.",
        commands: &[product(&["init"], &[])],
    }
    LayoutPreparedInstallation = "layout.prepared_installation" {
        axis: Layout, status: Info, basis: Observed,
        message: "This directory has the layout published by agentsandbox setup prepare: a tenant registry, an OIDC file and one private directory per tenant.",
        action: "No action is needed for the layout. The gateway reads this installation through its registry and its OIDC file.",
        commands: &[product(&["setup", "prepare"], &[])],
    }
    LayoutHandWrittenRegistry = "layout.hand_written_registry" {
        axis: Layout, status: Info, basis: Observed,
        message: "A tenant registry was given alone. Its references are resolved as the gateway resolves them: absolute paths as written, relative paths against the directory of the registry.",
        action: "No action is needed. Pass --oidc-config FILE as well to compare the registry with a capacity plan.",
        commands: &[tool(&["--oidc-config"])],
    }
    LayoutTargetAbsent = "layout.target_absent" {
        axis: Layout, status: Error, basis: Observed,
        message: "The inspected directory does not exist.",
        action: "For a local installation run agentsandbox init --config-dir {target}. For an SSO installation run agentsandbox setup prepare --input MANIFEST --output-dir NEW_DIR. If a preparation was interrupted, pass the retained staging directory that setup prepare printed as --installation-dir DIR.",
        variants: &[
            TextVariant {
                name: "default",
                message: Some("No target was given and the default directory .agentsandbox does not exist in the current directory."),
                action: Some("If an installation exists elsewhere, pass it with --config-dir DIR or --installation-dir DIR. To create one here: for a local installation run agentsandbox init --config-dir {target}; for an SSO installation run agentsandbox setup prepare --input MANIFEST --output-dir NEW_DIR. If a preparation was interrupted, pass the retained staging directory that setup prepare printed as --installation-dir DIR."),
            },
            TextVariant {
                name: "given",
                message: Some("The directory given on the command line does not exist."),
                action: Some("Check the path first: if the installation is elsewhere, run this tool again with that directory. To create an installation at this path: for a local installation run agentsandbox init --config-dir {target}; for an SSO installation run agentsandbox setup prepare --input MANIFEST --output-dir NEW_DIR. If a preparation was interrupted, pass the retained staging directory that setup prepare printed as --installation-dir DIR."),
            },
        ],
        commands: &[
            product(&["init"], &["--config-dir"]),
            product(&["setup", "prepare"], &["--input", "--output-dir"]),
            tool(&["--config-dir", "--installation-dir"]),
        ],
    }
    LayoutUnrecognized = "layout.unrecognized" {
        axis: Layout, status: Error, basis: Observed,
        message: "This directory exists but holds neither the historical layout nor a prepared installation: it is not an ai-guard installation.",
        action: "Give the directory that was passed to agentsandbox init, or the directory created by agentsandbox setup prepare. To create an installation in a new directory, run agentsandbox init --config-dir DIR for a local installation, or agentsandbox setup prepare --input MANIFEST --output-dir NEW_DIR for an SSO installation.",
        variants: &[
            TextVariant {
                name: "gateway_files",
                message: Some("This directory holds a tenant registry or an OIDC file at their fixed names, tenants.json and oidc.json, and neither the historical layout nor a setup record of a prepared installation. It is not inspected as an installation."),
                action: Some("To inspect these files as the gateway reads them, pass them with --tenants-file FILE and --oidc-config FILE instead of this directory."),
            },
            TextVariant {
                name: "gateway_files_registry_read",
                message: Some("This directory holds a tenant registry at its fixed name, tenants.json, and neither the historical layout nor a setup record of a prepared installation. Its name has the form of a setup staging directory, so the registry was read to tell what the directory is. The registry records no path outside this directory, so the directory is not taken for a staging directory, and it is not inspected as an installation."),
                action: Some("To inspect the registry as the gateway reads it, pass it with --tenants-file FILE, and an OIDC file with --oidc-config FILE, instead of this directory."),
            },
        ],
        commands: &[
            product(&["init"], &["--config-dir"]),
            product(&["setup", "prepare"], &["--input", "--output-dir"]),
            tool(&["--tenants-file", "--oidc-config"]),
        ],
    }
    LayoutPostureRefused = "layout.posture_refused" {
        axis: Layout, status: Error, basis: Observed,
        message: "The directory is refused by the private-directory rule of the control commands: it must be owned by the account that runs them, have mode 0700, and sit below directories that other accounts cannot rewrite, on a supported local file system.",
        action: "Use a private directory (mode 0700) on a supported local file system. Neither this tool nor the product repairs an existing path.",
        variants: &[
            TextVariant {
                name: "not_a_directory",
                message: Some("The given path is not a directory, or it is a symbolic link: the control commands open their directory without following a link."),
                action: Some("Give the directory itself, not a file inside it and not a link to it."),
            },
            TextVariant {
                name: "mode",
                message: Some("The directory does not have mode 0700. The private-directory rule of the control commands requires a directory owned by the account that runs them, with mode 0700, below directories that other accounts cannot rewrite, on a supported local file system."),
                action: Some("Give the directory mode 0700 if it is the private directory of the control account and no other account needs it. Neither this tool nor the product repairs an existing path."),
            },
            TextVariant {
                name: "above_or_file_system",
                message: Some("The directory is owned by the account running this tool and has mode 0700, and the private-directory rule of the control commands still refuses it: a directory above it can be written by group or others or belongs to another account than root or the control account, a component of its path is a symbolic link, or its file system is not a supported local one. On macOS, an extended access entry on the directory itself, or one that permits a modification on a directory above it, is a further cause."),
                action: Some("Check each directory above it with ls -ld, give its path without a symbolic link, and keep the installation on a supported local file system. Neither this tool nor the product repairs an existing path."),
            },
        ],
    }
    LayoutInspectorIdentityDiffers = "layout.inspector_identity_differs" {
        axis: Layout, status: Unknown, basis: Observed,
        message: "The directory belongs to another account than the one running this tool, so its content was not inspected.",
        action: "Run this tool as the control account that owns the installation.",
        variants: &[TextVariant {
            name: "unavailable",
            message: Some("The directory could not be examined from the account running this tool, so its content was not inspected."),
            action: None,
        }],
    }
    LayoutMixed = "layout.mixed" {
        axis: Layout, status: Error, basis: Observed,
        message: "The directory holds both the historical layout and a prepared installation: the historical initialisation was run over a prepared installation.",
        action: "Preserve the directory as it is. Do not run the historical initialisation or the provider initialisation on it again; use a separate directory as --config-dir DIR for the legacy commands.",
        commands: &[tool(&["--config-dir"])],
    }
    LayoutPreparedPassedAsConfigDir = "layout.prepared_passed_as_config_dir" {
        axis: Layout, status: Error, basis: Observed,
        message: "A prepared installation or a setup staging directory was given as the historical config directory. The legacy commands do not read this layout.",
        action: "Pass this directory as --installation-dir DIR.",
        commands: &[tool(&["--installation-dir"])],
    }
    LayoutHistoricalPassedAsInstallationDir = "layout.historical_passed_as_installation_dir" {
        axis: Layout, status: Error, basis: Observed,
        message: "A historical config directory was given as a prepared installation. It holds no tenant registry and no OIDC file for the gateway.",
        action: "Pass this directory as --config-dir DIR.",
        commands: &[tool(&["--config-dir"])],
    }
    LayoutLegacyCommandsDoNotReadThisLayout = "layout.legacy_commands_do_not_read_this_layout" {
        axis: Layout, status: Info, basis: Inferred,
        message: "The commands init, up, down, run, worker-status, worker-resume, worker-stop, export-workspace and providers read a historical config directory. The default verify-audit path also reads that layout; an explicit viewer file can select a prepared tenant for its online audit check.",
        action: "Use a separate historical config directory for those legacy commands. For a prepared tenant, agentsandbox verify-audit --viewer-token-file FILE --gateway URL reads the explicit viewer file. The gateway reads the installation through its registry and its OIDC file.",
        commands: &[product(&["verify-audit"], &["--viewer-token-file", "--gateway"])],
    }
    LayoutReferencesOutsideInspectedRoot = "layout.references_outside_inspected_root" {
        axis: Layout, status: Warning, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The files of this prepared installation record paths outside the inspected directory.",
        action: "The gateway started with these files reads the recorded paths, not the files in this directory. Run this tool where the installation is mounted at its recorded path, or rerun with --show-paths to see the recorded paths.",
        commands: &[product(&["gateway"], &[]), tool(&["--show-paths"])],
    }
    LayoutSetupStagingDirectory = "layout.setup_staging_directory" {
        axis: Layout, status: Error, basis: Observed,
        consumer: "agentsandbox setup resume",
        message: "This is a retained setup staging directory: the preparation was not published to its final path. Its files record the final paths, so the key files and the stores they name are not described in this report.",
        action: "Stop the controls, then run agentsandbox setup resume --recovery-dir {target} --confirm-control-stopped. Do not regenerate credentials.",
        commands: &[product(&["setup", "resume"], &["--recovery-dir", "--confirm-control-stopped"])],
    }
    LayoutStagingAndFinalBothExist = "layout.staging_and_final_both_exist" {
        axis: Layout, status: Error, basis: Observed,
        consumer: "agentsandbox setup resume",
        message: "This setup staging directory and a path recorded by its registry both exist. The key files and the stores at the recorded paths are not described in this report.",
        action: "Preserve both directories and do not resume: the product refuses to publish over an existing destination. Establish which one holds the installation before any other step.",
        commands: &[product(&["setup", "resume"], &[])],
    }
    LayoutStagingWithoutIntent = "layout.staging_without_intent" {
        axis: Layout, status: Error, basis: Observed,
        consumer: "agentsandbox setup resume",
        message: "This directory has the name of a setup staging directory but holds no preparation record. Nothing it holds is described in this report.",
        action: "Preserve it for diagnosis and prepare into a different new destination with agentsandbox setup prepare --input MANIFEST --output-dir NEW_DIR.",
        commands: &[
            product(&["setup", "resume"], &[]),
            product(&["setup", "prepare"], &["--input", "--output-dir"]),
        ],
    }
    LayoutCompletionMarkerAbsent = "layout.completion_marker_absent" {
        axis: Layout, status: Warning, basis: Observed,
        consumer: "agentsandbox setup resume",
        message: "This published installation has no completion marker. The product writes the marker before publication, so it was removed afterwards.",
        action: "Stop the controls, then run agentsandbox setup resume --recovery-dir {target} --confirm-control-stopped so that the product checks the retained preparation. Do not regenerate credentials.",
        commands: &[product(&["setup", "resume"], &["--recovery-dir", "--confirm-control-stopped"])],
    }

    // ---- topology ---------------------------------------------------------
    TopologySingleLocalTenant = "topology.single_local_tenant" {
        axis: Topology, status: Info, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "A gateway started without a tenant registry serves one tenant, named local.",
        action: "No action is needed. For several tenants, start the gateway with a tenant registry.",
        commands: &[product(&["gateway"], &[])],
    }
    TopologyRegistryTenants = "topology.registry_tenants" {
        axis: Topology, status: Info, basis: Declared,
        consumer: "agentsandbox gateway",
        message: "The tenant registry declares the tenants counted in this report. Each tenant has its own store and its own technical keys.",
        action: "No action is needed.",
        commands: &[product(&["gateway"], &[])],
    }
    TopologyRunningServicesNotChecked = "topology.running_services_not_checked" {
        axis: Topology, status: NotChecked, basis: Observed,
        message: "This tool starts nothing and contacts nothing: whether the gateway, the dashboard or a worker is running is not checked.",
        action: "Ask a running service with agentsandbox healthcheck --url URL.",
        requires: "agentsandbox healthcheck --url URL",
        variants: &[TextVariant {
            name: "after_active_check",
            message: Some("This tool starts nothing. The active check of this run contacted the identity provider and nothing else: whether the gateway, the dashboard or a worker is running is not checked."),
            action: None,
        }],
        commands: &[product(&["healthcheck"], &["--url"])],
    }
    TopologyGatewayArgumentsNotPersisted = "topology.gateway_arguments_not_persisted" {
        axis: Topology, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The arguments of the gateway and of the dashboard are not stored in the installation: listening addresses, policy file and event bound are not checked.",
        action: "Read them in the command line or the Compose file of the launcher.",
        requires: "the command line or the Compose file of the launcher",
        commands: &[product(&["gateway"], &[])],
    }

    // ---- identity ---------------------------------------------------------
    IdentitySsoNotInspected = "identity.sso_not_inspected" {
        axis: Identity, status: NotChecked, basis: Observed,
        message: "No OIDC file was given, so single sign-on was not inspected. SSO is optional and is enabled only by a gateway option; a historical config directory alone says nothing about it.",
        action: "Pass --oidc-config FILE or --installation-dir DIR to inspect an OIDC file.",
        requires: "--oidc-config FILE or --installation-dir DIR",
        variants: &[TextVariant {
            name: "staging",
            message: Some("The setup staging directory holds no OIDC file yet, so single sign-on was not inspected."),
            action: Some("Complete the preparation first, then pass the published directory as --installation-dir DIR."),
        }],
        commands: &[tool(&["--oidc-config", "--installation-dir"])],
    }
    IdentitySsoFileValidOffline = "identity.sso_file_valid_offline" {
        axis: Identity, status: Ok, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The structure and the policies of the OIDC file pass the offline rules of the gateway loader, restated in this tool. The verdict is drawn from the file alone.",
        action: "No action is needed for the file. Read the items listed as not checked before relying on it.",
        commands: &[product(&["gateway"], &[])],
    }
    IdentitySsoGatewayEnablementNotPersisted = "identity.sso_gateway_enablement_not_persisted" {
        axis: Identity, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "A valid OIDC file does not prove that the gateway is started with it: SSO is enabled only by the gateway option --oidc-config.",
        action: "Check the command line of the gateway launcher.",
        requires: "the command line of the gateway launcher",
        commands: &[product(&["gateway"], &["--oidc-config"])],
    }
    IdentitySsoDemonstrationTransport = "identity.sso_demonstration_transport" {
        axis: Identity, status: Warning, basis: Declared,
        consumer: "agentsandbox gateway",
        message: "The OIDC file declares the isolated demonstration transport: sign-in talks to the identity provider in plain HTTP. The product limits this transport to a literal loopback issuer and to the distributed demonstration issuer.",
        action: "Keep such an installation for trials. For a normal installation use the https_only transport with an HTTPS issuer.",
        commands: &[product(&["gateway"], &[])],
    }
    IdentityLocalNamedAccountsNotAvailable = "identity.local_named_accounts_not_available" {
        axis: Identity, status: NotAvailable, basis: Observed,
        message: "Local named accounts and memberships live in the tenant stores, which this build cannot inspect offline.",
        action: "Nothing in this build covers it offline.",
    }
    IdentityIdpReachabilityNotChecked = "identity.idp_reachability_not_checked" {
        axis: Identity, status: NotChecked, basis: Observed,
        message: "The identity provider was not contacted: this tool opens no connection unless an active check is asked for. The gateway contacts the provider when a sign-in starts.",
        action: "To ask the provider for its discovery document and its keys from where this tool runs, pass --active oidc-discovery. Two requests are sent, and no credential.",
        requires: "--active oidc-discovery, with an OIDC file that passes the offline rules",
        variants: &[
            TextVariant {
                name: "not_run",
                message: Some("The active check was asked for, but the OIDC file does not pass the offline rules, so no client was built from it and nothing was contacted."),
                action: Some("Correct what this axis reports for the OIDC file, then run the same command again."),
            },
            TextVariant {
                name: "no_file",
                message: Some("The active check was asked for, but no OIDC file was inspected, so there is no identity provider to contact. Nothing was contacted."),
                action: Some("Pass --oidc-config FILE or --installation-dir DIR together with --active oidc-discovery."),
            },
        ],
        commands: &[tool(&["--active", "--oidc-config", "--installation-dir"])],
    }
    ActiveOidcDiscoveryOk = "active.oidc_discovery_ok" {
        axis: Identity, status: Ok, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The identity provider answered the discovery of the product client. Its discovery document confirms the issuer and the endpoints of the OIDC file and announces the code flow, RS256 signatures and S256 proof keys; its key set holds a usable signing key. Two GET requests were sent from where this tool runs, and no credential with them: the first for the discovery document below the issuer, the second to the keys endpoint.",
        action: "No action is needed. This proves neither a sign-in, nor that the gateway reaches the provider from its own network.",
        commands: &[product(&["gateway"], &[])],
    }
    ActiveOidcDiscoveryTransportUnavailable = "active.oidc_discovery_transport_unavailable" {
        axis: Identity, status: Unknown, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The identity provider gave no usable answer from where this tool runs, within the bounds of the OIDC file: the connection or the TLS handshake failed, the time limit passed, or an answer exceeded the size bound. The product client gives no further detail, and none is printed.",
        action: "Check that the issuer can be reached from this host and that the CA bundle of the OIDC file covers its certificate. The gateway may reach the provider from another network: run this check where the gateway runs.",
        variants: &[TextVariant {
            name: "not_completed",
            message: Some("The discovery did not complete, for a reason that is not an answer of the identity provider. Nothing is concluded about the provider."),
            action: Some("Run the same command again. If it persists, report a defect with the command line only."),
        }],
        commands: &[product(&["gateway"], &[])],
    }
    ActiveOidcDiscoveryProviderRejected = "active.oidc_discovery_provider_rejected" {
        axis: Identity, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The identity provider answered, but not with the JSON documents of a discovery: a status other than 200, another content type, or a body that is not unambiguous JSON. Its answer is not printed.",
        action: "Compare the issuer and the keys endpoint of the OIDC file with the configuration of the identity provider.",
        commands: &[product(&["gateway"], &[])],
    }
    ActiveOidcDiscoveryMetadataMismatch = "active.oidc_discovery_metadata_mismatch" {
        axis: Identity, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The discovery document of the identity provider does not confirm the OIDC file: the issuer, the authorization endpoint, the endpoint that issues tokens, the keys endpoint or a declared end-session endpoint differs, or the provider does not announce the code flow, RS256 signatures and S256 proof keys. Its content is not printed.",
        action: "Compare the endpoints of the OIDC file with the discovery document of the provider. The product accepts exact values only and never adopts an endpoint from a discovery.",
        commands: &[product(&["gateway"], &[])],
    }
    ActiveOidcDiscoveryKeysInvalid = "active.oidc_discovery_keys_invalid" {
        axis: Identity, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The key set of the identity provider is refused by the product client: no usable RS256 signing key, a missing or duplicate key identifier, private key material, or more keys than the bound of the OIDC file. Its content is not printed.",
        action: "Check the keys endpoint of the OIDC file and the signing keys of the identity provider.",
        commands: &[product(&["gateway"], &[])],
    }
    OidcFileAbsent = "oidc.file_absent" {
        axis: Identity, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The OIDC file does not exist at the given path.",
        action: "Give the file that is passed to the gateway as --oidc-config, or pass the directory created by agentsandbox setup prepare as --installation-dir DIR.",
        commands: &[
            product(&["gateway"], &["--oidc-config"]),
            product(&["setup", "prepare"], &[]),
            tool(&["--installation-dir"]),
        ],
    }
    OidcFileUnavailableOrNotPrivate = "oidc.file_unavailable_or_not_private" {
        axis: Identity, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The OIDC file is refused by the private-file rule of the gateway: a regular file owned by the control account, mode 0600, one link.",
        action: "Restore a private file (mode 0600, single link) owned by the control account. The file was not read.",
        variants: &[
            TextVariant {
                name: "reserved",
                message: Some("The given OIDC file is a file that this installation uses as a key file, a setup record or a store, or lies below a directory that holds only such files. It was not opened."),
                action: Some("Give the OIDC file itself, as it is passed to the gateway as --oidc-config."),
            },
            TextVariant {
                name: "reserved_name",
                message: Some("The OIDC file of this installation directory is a second name of a file that this installation uses as a key file, a setup record or a store. It was not opened."),
                action: Some("Restore the OIDC file of the installation under its own name. A second name of a key file, a setup record or a store must not stand in its place. The gateway reads the file given to it as --oidc-config."),
            },
        ],
        commands: &[product(&["gateway"], &["--oidc-config"])],
    }
    OidcJsonInvalid = "oidc.json_invalid" {
        axis: Identity, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The OIDC file is not the closed JSON document the gateway expects. Only the error category and its position are reported.",
        action: "Correct the file at the reported line and column. Unknown fields are refused.",
        commands: &[product(&["gateway"], &[])],
    }
    OidcVersionPlanPairInvalid = "oidc.version_plan_pair_invalid" {
        axis: Identity, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The OIDC file version and its capacity plan do not go together: version 1 carries no plan, version 2 requires one.",
        action: "Use version 1 without a capacity plan for a single tenant, or version 2 with a capacity plan. For an installation that has already started, follow the capacity migration of the SSO guide with the consumers stopped.",
        commands: &[product(&["gateway"], &[])],
    }
    OidcGatewayBlockInvalid = "oidc.gateway_block_invalid" {
        axis: Identity, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The gateway block of the OIDC file is refused by the product validator. The reason is a closed code; the text of the validator is not printed.",
        action: "Correct the fields listed under basis. The SSO guide states their rules; the diagnostics guide names its section for each reason.",
        variants: &[TextVariant {
            name: "provider_object",
            message: Some("The provider object of the gateway block is refused by the product validator, which returns one closed value for every rule of that object: its version, distinct client identifiers and audience, issuer and endpoint schemes for the declared transport, distinct endpoints, redirect, callback path, logout returns and the limits. Which rule failed is not known; the text of the validator is not printed."),
            action: None,
        }],
        commands: &[product(&["gateway"], &[])],
    }
    OidcCapacityPlanInvalid = "oidc.capacity_plan_invalid" {
        axis: Identity, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The capacity plan of the OIDC file is refused by the product validator.",
        action: "Correct the capacity plan: its version, the unscoped share and one finite share per tenant, as the SSO guide describes.",
        commands: &[product(&["gateway"], &[])],
    }
    OidcAuthenticationPolicyInvalid = "oidc.authentication_policy_invalid" {
        axis: Identity, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The authentication audit policy of the OIDC file is refused by a product validator of the authentication audit: the policy check that the gateway applies before it opens or creates the audit, or, for a version 2 file, the check of setup that the policy funds the capacity plan. The gateway refuses a file with either defect; for the second, only after it has opened or created the audit.",
        action: "Correct the authentication audit policy as the SSO guide describes. Keep the original policy of an installation that has already started.",
        commands: &[product(&["gateway"], &[])],
    }
    OidcReferenceInvalid = "oidc.reference_invalid" {
        axis: Identity, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "A file reference of the OIDC file is empty or leaves its directory by parent traversal.",
        action: "Write a non-empty reference: absolute, or relative to the directory of the OIDC file without parent traversal.",
        commands: &[product(&["gateway"], &[])],
    }
    OidcLoaderAcceptanceNotChecked = "oidc.loader_acceptance_not_checked" {
        axis: Identity, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "Whether the gateway accepts this OIDC file when it starts is not checked: it also compares the file with the durable authentication state.",
        action: "Start the gateway; it refuses to start when the durable state and the file disagree.",
        requires: "the gateway checks the stores and the durable authentication state when it starts",
        commands: &[product(&["gateway"], &[])],
    }

    // ---- tenants ----------------------------------------------------------
    RegistryFileUnreadable = "registry.file_unreadable" {
        axis: Tenants, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The tenant registry cannot be read as the gateway reads it: it must be a regular file that the account of the gateway can read, not a symbolic link, of at most 1 MiB.",
        action: "Give the registry file that is passed to the gateway as --tenants-file.",
        variants: &[
            TextVariant {
                name: "absent",
                message: Some("The tenant registry does not exist at the given path."),
                action: None,
            },
            TextVariant {
                name: "absent_from_installation",
                message: Some("This installation directory holds no tenants.json: the tenant registry that setup prepare published is absent."),
                action: Some("Restore the tenants.json of this installation at its original path, from your backup, then run this tool again."),
            },
            TextVariant {
                name: "reserved",
                message: Some("The given tenant registry is a file that this installation uses as a key file, a setup record or a store, or lies below a directory that holds only such files. It was not opened."),
                action: None,
            },
            TextVariant {
                name: "reserved_name",
                message: Some("The tenant registry of this installation directory is a second name of a file that this installation uses as a key file, a setup record or a store. It was not opened."),
                action: Some("Restore the tenant registry of the installation under its own name. A second name of a key file, a setup record or a store must not stand in its place. The gateway reads the file given to it as --tenants-file."),
            },
        ],
        commands: &[product(&["gateway"], &["--tenants-file"])],
    }
    RegistryJsonInvalid = "registry.json_invalid" {
        axis: Tenants, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The tenant registry is not the closed JSON document the gateway expects. Only the error category and its position are reported.",
        action: "Correct the file at the reported line and column. Unknown fields are refused.",
        commands: &[product(&["gateway"], &[])],
    }
    RegistryVersionUnknown = "registry.version_unknown" {
        axis: Tenants, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The tenant registry version is not 1, the only version the gateway reads.",
        action: "Use registry version 1.",
        commands: &[product(&["gateway"], &[])],
    }
    RegistryEmpty = "registry.empty" {
        axis: Tenants, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The tenant registry declares no tenant.",
        action: "Declare at least one tenant.",
        commands: &[product(&["gateway"], &[])],
    }
    RegistryTenantIdInvalid = "registry.tenant_id_invalid" {
        axis: Tenants, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "A tenant identifier is refused by the product validator: 1 to 128 ASCII letters, digits, hyphens or underscores.",
        action: "Correct the listed identifiers in the registry.",
        commands: &[product(&["gateway"], &[])],
    }
    RegistryTenantIdDuplicate = "registry.tenant_id_duplicate" {
        axis: Tenants, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "A tenant identifier appears more than once in the registry.",
        action: "Keep one entry per tenant.",
        commands: &[product(&["gateway"], &[])],
    }
    RegistryReferenceEmpty = "registry.reference_empty" {
        axis: Tenants, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "A tenant entry has an empty store reference or an empty key file reference.",
        action: "Write the three references of each listed tenant: its store and its two key files.",
        commands: &[product(&["gateway"], &[])],
    }
    RegistryStorageInvalid = "registry.storage_invalid" {
        axis: Tenants, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "A tenant must declare either data or a valid storage profile.",
        action: "Declare one storage selector. For MySQL, provide the database, account, namespace, host, port and the TLS and credential references required by its authentication mode.",
        commands: &[product(&["gateway"], &["--tenants-file"])],
    }
    RegistryMySqlUnavailable = "registry.mysql_unavailable" {
        axis: Tenants, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The tenant declares MySQL storage. This gateway lacks the MySQL transactions required to serve it.",
        action: "Keep the configured destination and use a gateway that supports its MySQL storage profile.",
        commands: &[product(&["gateway"], &["--tenants-file"])],
    }
    RegistryPolicyInvalid = "registry.policy_invalid" {
        axis: Tenants, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "A tenant policy is refused by the product validator. An audit-capacity or a workload-audit policy is judged for the gateway event bound given with --max-events.",
        action: "Correct the policy named in the details for the listed tenants. If the gateway is started with another event bound, give that value with --max-events N.",
        commands: &[product(&["gateway"], &["--max-events"]), tool(&["--max-events"])],
    }
    RegistryPolicyRefusedAtAssumedEventBound = "registry.policy_refused_at_assumed_event_bound" {
        axis: Tenants, status: Unknown, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "A tenant policy whose verdict depends on the gateway event bound is refused by the product validator at 100000 events. That bound is the gateway default, assumed by this tool because --max-events was not given. A gateway started with another bound may accept the policy.",
        action: "Give the event bound of the gateway with --max-events N to get its verdict. A gateway started without --max-events uses this default and refuses the policy; in that case, correct the policy named in the details for the listed tenants.",
        commands: &[product(&["gateway"], &["--max-events"]), tool(&["--max-events"])],
    }
    RegistryEventBoundNotUsed = "registry.event_bound_not_used" {
        axis: Tenants, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "A gateway event bound was given with --max-events, and nothing in this run was validated with it: no decoded registry declares an audit-capacity or a workload-audit policy, the only tenant policies whose verdict depends on the bound. A registry written by setup prepare declares none.",
        action: "No action is needed for the registry. The event bound of the gateway is one of its arguments, which are not stored in the installation.",
        requires: "a registry whose tenants declare an audit-capacity or a workload-audit policy",
        commands: &[product(&["gateway"], &["--max-events"]), tool(&["--max-events"])],
    }
    AuditLiveActivityBudgetCapacity = "audit.live_activity_budget_capacity" {
        axis: Tenants, status: Warning, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "With the per-session live provider activity budget, a tenant audit holds fewer than 50 sessions: the gateway event bound divided by the event budget, or the 256 MiB physical audit cap divided by the byte budget, is below 50. The audit has no rotation yet and refuses every write once full, launches included.",
        action: "Give this tool the budget of the broker registry with --activity-max-events and --activity-max-bytes, raise the event bound of agentsandbox gateway with --max-events, or lower the provider_activity budget of the broker registry.",
        commands: &[product(&["gateway"], &["--max-events"]), tool(&["--max-events", "--activity-max-events", "--activity-max-bytes"])],
    }
    RegistryOfflineRulesPassed = "registry.offline_rules_passed" {
        axis: Tenants, status: Ok, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The structure and the policies of the tenant registry pass the offline rules of the gateway loader, restated in this tool. Token content is not checked.",
        action: "No action is needed for the file. Read the items listed as not checked before relying on it.",
        commands: &[product(&["gateway"], &[])],
    }
    RegistryLoaderAcceptanceNotChecked = "registry.loader_acceptance_not_checked" {
        axis: Tenants, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "Whether the gateway accepts this registry when it starts is not checked: it also reads every key file, requires distinct keys and opens every store.",
        action: "Start the gateway; it refuses to start when a key file or a store is unusable.",
        requires: "the gateway checks token content, token uniqueness and the stores when it starts",
        commands: &[product(&["gateway"], &[])],
    }
    TenantsPlanMatchesRegistry = "tenants.plan_matches_registry" {
        axis: Tenants, status: Ok, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The capacity plan and the tenant registry name exactly the same tenants.",
        action: "No action is needed for the files. The comparison with the durable plan is listed separately.",
        commands: &[product(&["gateway"], &[])],
    }
    TenantsPlanSetMismatch = "tenants.plan_set_mismatch" {
        axis: Tenants, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The capacity plan and the tenant registry name different tenants.",
        action: "Restore the registry to the tenant set of the capacity plan; the details list each side. Once an authentication audit of format 4 exists, its stored plan cannot be changed: a version 2 preparation creates that audit, the gateway refuses a file whose plan differs from the stored one, and adding or removing a tenant is not implemented (SSO guide, Allocate authentication capacity). The plan of the file can be changed only while no audit of format 4 exists: before the audit of a hand-written file is first initialised, or for the 3 to 4 migration of the SSO guide, with the consumers stopped. Never initialise a replacement audit.",
        commands: &[product(&["gateway"], &[])],
    }
    TenantsPlanRequiredForMultiTenant = "tenants.plan_required_for_multi_tenant" {
        axis: Tenants, status: Error, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "The registry declares several tenants but the OIDC file is version 1, without a capacity plan. A multi-tenant OIDC gateway refuses this pair.",
        action: "A multi-tenant OIDC gateway needs authentication audit format 4 and a version 2 file with a capacity plan. A version 1 preparation creates format 3: follow the 3 to 4 migration of the SSO guide before the first start, with the consumers stopped, a new backup and the original policy.",
        commands: &[product(&["gateway"], &[])],
    }
    TenantsSingleTenantWithoutPlan = "tenants.single_tenant_without_plan" {
        axis: Tenants, status: Ok, basis: Inferred,
        consumer: "agentsandbox gateway",
        message: "One tenant and a version 1 OIDC file without a capacity plan: the two files are compatible with each other.",
        action: "No action is needed for the files. The gateway also refuses a version 1 file over an authentication audit of format 4; that comparison is listed separately.",
        commands: &[product(&["gateway"], &[])],
    }
    TenantsCorrespondenceNotChecked = "tenants.correspondence_not_checked" {
        axis: Tenants, status: NotChecked, basis: Observed,
        message: "Only one of the tenant registry and the OIDC file was inspected, so the tenant set and the capacity plan were not compared.",
        action: "Pass both --tenants-file FILE and --oidc-config FILE, or --installation-dir DIR.",
        requires: "both --tenants-file FILE and --oidc-config FILE, or --installation-dir DIR",
        variants: &[
            TextVariant {
                name: "undecoded",
                message: Some("The tenant registry or the OIDC file could not be decoded, so the tenant set and the capacity plan were not compared."),
                action: Some("Correct the file reported as invalid or unreadable, then run this tool again with both files."),
            },
            TextVariant {
                name: "single_local",
                message: Some("A historical config directory and an OIDC file were inspected without a tenant registry, so the tenant set and the capacity plan were not compared."),
                action: Some("A gateway started without a registry serves the single tenant local: a version 1 file needs no capacity plan, and the plan of a version 2 file must name exactly local. If the gateway is started with a registry, pass it with --tenants-file FILE."),
            },
        ],
        commands: &[tool(&["--tenants-file", "--oidc-config", "--installation-dir"])],
    }
    TenantsDurablePlanComparisonNotAvailable = "tenants.durable_plan_comparison_not_available" {
        axis: Tenants, status: NotAvailable, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The gateway also compares the capacity plan with the plan stored in the authentication audit. This build cannot read that store offline.",
        action: "Nothing in this build covers it offline. The gateway makes the comparison when it starts and refuses to start on a difference.",
        commands: &[product(&["gateway"], &[])],
    }

    // ---- credential references --------------------------------------------
    ReferenceAbsent = "reference.absent" {
        axis: CredentialReferences, status: Error, basis: Observed,
        message: "A referenced file does not exist at its recorded path, inside the inspected directory or beside the registry.",
        action: "Restore the file at its recorded path or correct the reference. Rerun with --show-paths to see the recorded path.",
        variants: &[TextVariant {
            name: "historical_key",
            message: Some("A key of this historical config directory does not exist at its fixed path."),
            action: Some("Restore the file from your backup, or run agentsandbox init --config-dir {target}: it creates the missing key and preserves the existing ones. A recreated key is a new value: restart the services and the clients that held the previous one."),
        }],
        commands: &[
            tool(&["--show-paths"]),
            product(&["init"], &["--config-dir"]),
        ],
    }
    ReferenceNotFoundFromHere = "reference.not_found_from_here" {
        axis: CredentialReferences, status: Unknown, basis: Observed,
        message: "A referenced path was not found from where this tool runs. The path is outside the inspected directory, so it may exist where its consumer runs.",
        action: "If the gateway runs in a container or on another host, run this tool there. Rerun with --show-paths to see the recorded path.",
        commands: &[tool(&["--show-paths"])],
    }
    ReferenceNotARegularFile = "reference.not_a_regular_file" {
        axis: CredentialReferences, status: Error, basis: Observed,
        message: "A referenced path is a symbolic link or is not a regular file. Every reader of the product opens these files without following links and requires a regular file.",
        action: "Replace the entry by a regular file at the recorded path.",
    }
    ReferencePrivateRuleViolated = "reference.private_rule_violated" {
        axis: CredentialReferences, status: Error, basis: Inferred,
        message: "A file read through the private-file rule has a mode other than 0600, more than one link, or access rights granted to another account. Its consumer refuses it for certain.",
        action: "Restore mode 0600 and a single link on the listed file, owned by the control account, without extended access entries.",
        variants: &[TextVariant {
            name: "path",
            message: Some("A file read through the private-file rule is refused by its reader for its path: a directory that leads to the file can be written by group or others, belongs to another account than root or the control account, is a symbolic link, cannot be opened, or is on an unsupported file system; on macOS, such a directory can also carry an extended access entry that permits a modification. Its consumer refuses it for certain. The verdict is that of the product walk, which this tool asked without opening the file."),
            action: Some("Keep the listed file below directories that only root or the control account can write, without a symbolic link on the way."),
        }],
    }
    ReferencePrivateRuleNotEvaluated = "reference.private_rule_not_evaluated" {
        axis: CredentialReferences, status: NotChecked, basis: Observed,
        message: "A file read through the private-file rule conforms by its metadata: a regular file of mode 0600 with one link, owned by the account running this tool. The product walk that its reader applies to the directories leading to it accepts them. Its own directory is not a private directory of mode 0700, so the entry check of the product did not run here: the file system and the extended access entries of the file itself are not checked.",
        action: "The consuming command checks them when it opens the file. To have the entry check run in this tool, keep the file in a directory of mode 0700 owned by the control account.",
        requires: "the consuming command checks the file system and the access entries of the file when it opens it",
    }
    ReferenceTokenFileAccessibleToOtherAccounts = "reference.token_file_accessible_to_other_accounts" {
        axis: CredentialReferences, status: Warning, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The gateway token reader checks no mode: it reads any regular file that the account of the gateway can open for reading. The mode of this file grants access to its group or to the other accounts of this host. This is expected for a read-only projected secret inside a container.",
        action: "On a shared host, restrict the file to the control account (mode 0600). Keep the projection mode where the file is a container secret.",
        commands: &[product(&["gateway"], &[])],
    }
    ReferenceInspectorIdentityDiffers = "reference.inspector_identity_differs" {
        axis: CredentialReferences, status: Unknown, basis: Observed,
        message: "A referenced file belongs to another account than the one running this tool. Its consumer may run as the owner, so the private-file rule was not judged.",
        action: "Run this tool as the control account.",
        variants: &[TextVariant {
            name: "registry",
            message: Some("The tenant registry belongs to another account than the one running this tool, which cannot read it. A gateway that runs as its owner may read it, so nothing is concluded about the file."),
            action: None,
        }],
    }
    ReferenceMetadataUnavailable = "reference.metadata_unavailable" {
        axis: CredentialReferences, status: Unknown, basis: Observed,
        message: "The metadata of a referenced path could not be read, for a reason other than absence.",
        action: "Check the permissions of the directories that lead to the path, then run this tool as the control account.",
    }
    ReferenceContentNotChecked = "reference.content_not_checked" {
        axis: CredentialReferences, status: NotChecked, basis: Observed,
        message: "This tool opens no file that the inspected files name as a key file, a token file or a credential file. Whether each file holds a well-formed and distinct value is not checked.",
        action: "The consuming command validates the content when it starts.",
        requires: "the consuming command checks the content when it starts",
        variants: &[TextVariant {
            name: "after_read",
            message: Some("Apart from the file that reference.file_read_before_it_was_named reports, this tool opened no file that the inspected files name as a key file, a token file or a credential file. Whether each file holds a well-formed and distinct value is not checked."),
            action: None,
        }],
    }
    ReferenceSetupSecretMaterialPresent = "reference.setup_secret_material_present" {
        axis: CredentialReferences, status: Info, basis: Observed,
        consumer: "agentsandbox setup resume",
        message: "The setup recovery record that holds the generated tokens is present. It is never displayed and was not opened.",
        action: "Keep it private with the installation; setup recovery needs it.",
        commands: &[product(&["setup", "resume"], &[])],
    }
    ReferenceFileReadBeforeItWasNamed = "reference.file_read_before_it_was_named" {
        axis: CredentialReferences, status: Error, basis: Observed,
        message: "This run opened a file as the input that its option or its fixed name describes, and a file read after it names that same file as a key file, a credential, a setup record or a store. Its content was read before that could be known. It is not printed. Where this report lists that file, its entry says content read_before_named.",
        action: "Give each option the file it describes, then run this tool again. Treat the content of that file as read by the account that ran this tool.",
    }

    // ---- providers --------------------------------------------------------
    ProviderNotInspected = "provider.not_inspected" {
        axis: Providers, status: NotChecked, basis: Observed,
        message: "No provider location was given, so the AI client profiles were not inspected.",
        action: "Pass --config-dir DIR or --providers-file FILE.",
        requires: "--config-dir DIR or --providers-file FILE",
        variants: &[TextVariant {
            name: "target",
            message: Some("The config directory could not be inspected, so the AI client profiles were not inspected."),
            action: Some("Correct what the layout axis reports for the config directory, or pass --providers-file FILE."),
        }],
        commands: &[tool(&["--config-dir", "--providers-file"])],
    }
    ProviderFileAbsent = "provider.file_absent" {
        axis: Providers, status: Info, basis: Observed,
        consumer: "agentsandbox run",
        message: "This config directory has no providers.json: no managed AI client profile is declared.",
        action: "To declare API profiles, run agentsandbox providers init --config-dir {target} --env-file PATH. Subscription profiles are written by hand, as the provider guide describes.",
        variants: &[TextVariant {
            name: "not_historical",
            message: None,
            action: Some("Declare provider profiles in the historical config directory that the launch commands use, not in this directory."),
        }],
        commands: &[
            product(&["run"], &[]),
            product(&["providers", "init"], &["--config-dir", "--env-file"]),
        ],
    }
    ProviderFileUnavailableOrNotPrivate = "provider.file_unavailable_or_not_private" {
        axis: Providers, status: Error, basis: Observed,
        consumer: "agentsandbox run",
        message: "The provider profile file is refused by the private-file rule: a regular file owned by the control account, mode 0600, one link, at most 65536 bytes.",
        action: "Restore a private file (mode 0600, single link). The file was not read.",
        variants: &[
            TextVariant {
                name: "absent",
                message: Some("The provider profile file given on the command line does not exist."),
                action: Some("Give the file that is passed to agentsandbox run as --providers-file."),
            },
            TextVariant {
                name: "reserved",
                message: Some("The given provider profile file is a file that this installation uses as a key file, a setup record or a store, or lies below a directory that holds only such files. It was not opened."),
                action: Some("Give the provider profile file itself, as it is passed to agentsandbox run with --providers-file."),
            },
        ],
        commands: &[product(&["run"], &["--providers-file"])],
    }
    ProviderConfigurationValid = "provider.configuration_valid" {
        axis: Providers, status: Ok, basis: Observed,
        consumer: "agentsandbox run",
        message: "The provider profile file is accepted by the product validator, which reads that file alone.",
        action: "No action is needed for the file. Read the items listed as not checked before a launch.",
        commands: &[product(&["run"], &[])],
    }
    ProviderConfigurationInvalid = "provider.configuration_invalid" {
        axis: Providers, status: Error, basis: Observed,
        consumer: "agentsandbox run",
        message: "The provider profile file is refused by the product validator. The reason is a closed code; the text of the validator is not printed.",
        action: "Correct the field named by the reason. Unknown fields, such as a URL or a CA field, are refused: this build has no managed internal-inference profile.",
        commands: &[product(&["run"], &[])],
    }
    ProviderConfigurationAmbiguous = "provider.configuration_ambiguous" {
        axis: Providers, status: Error, basis: Observed,
        consumer: "agentsandbox run",
        message: "The provider profile file is ambiguous: a duplicate profile, an unknown or duplicate priority, or a default profile that does not exist.",
        action: "Make profile identifiers unique, list only declared profiles in auto_priority, and name a declared profile as default_profile.",
        commands: &[product(&["run"], &[])],
    }
    ProviderLoginRequired = "provider.login_required" {
        axis: Providers, status: Error, basis: Inferred,
        consumer: "agentsandbox run",
        message: "The profile that the requested selection resolves is a subscription profile and its credential file does not exist. A managed launch with this selection refuses with login_required and never tries an API profile.",
        action: "Sign in with the client into its dedicated store, as the provider guide describes, then check that the profile references that file. No API profile is used in its place.",
        commands: &[product(&["run"], &[])],
    }
    ProviderCredentialMissing = "provider.credential_missing" {
        axis: Providers, status: Error, basis: Inferred,
        consumer: "agentsandbox run",
        message: "The profile that the requested selection resolves is an API profile and its credential file does not exist. A managed launch with this selection refuses with credential_missing.",
        action: "Restore the referenced env file or correct the reference. Rerun with --show-paths to see the recorded path.",
        commands: &[product(&["run"], &[]), tool(&["--show-paths"])],
    }
    ProviderCredentialUnsafe = "provider.credential_unsafe" {
        axis: Providers, status: Error, basis: Inferred,
        consumer: "agentsandbox run",
        message: "The credential file of the profile that the requested selection resolves is a symbolic link, is not a regular file, or violates the private-file rule. A managed launch with this selection refuses it.",
        action: "Replace it by a private regular file (mode 0600, single link) owned by the control account.",
        variants: &[
            TextVariant {
                name: "below_a_non_directory",
                message: Some("The credential path of the profile that the requested selection resolves passes through an entry that is not a directory, so no file can exist at that path. A managed launch with this selection refuses it as unsafe, not as missing."),
                action: Some("Correct the credential path of the profile so that every component before the file name is a directory. Rerun with --show-paths to see the recorded path."),
            },
            TextVariant {
                name: "path",
                message: Some("The credential file of the profile that the requested selection resolves is refused by its reader for its path: a directory that leads to the file can be written by group or others, belongs to another account than root or the control account, is a symbolic link, cannot be opened, or is on an unsupported file system; on macOS, such a directory can also carry an extended access entry that permits a modification. A managed launch with this selection refuses it. The verdict is that of the product walk, which this tool asked without opening the file."),
                action: Some("Keep the file below directories that only root or the control account can write, without a symbolic link on the way. Rerun with --show-paths to see the recorded path."),
            },
        ],
        commands: &[product(&["run"], &[]), tool(&["--show-paths"])],
    }
    ProviderUnselectedProfileReferenceAbsent = "provider.unselected_profile_reference_absent" {
        axis: Providers, status: Warning, basis: Inferred,
        consumer: "agentsandbox run",
        message: "The credential file of a declared profile does not exist. The requested selection does not resolve this profile; a launch that selects it refuses.",
        action: "Restore the file or remove the profile. Rerun with --show-paths to see the recorded path.",
        commands: &[product(&["run"], &[]), tool(&["--show-paths"])],
    }
    ProviderUnselectedProfileReferenceUnsafe = "provider.unselected_profile_reference_unsafe" {
        axis: Providers, status: Warning, basis: Inferred,
        consumer: "agentsandbox run",
        message: "The credential file of a declared profile is a symbolic link, is not a regular file, or violates the private-file rule. The requested selection does not resolve this profile; a launch that selects it refuses.",
        action: "Replace it by a private regular file (mode 0600, single link) owned by the control account, or remove the profile.",
        variants: &[
            TextVariant {
                name: "below_a_non_directory",
                message: Some("The credential path of a declared profile passes through an entry that is not a directory, so no file can exist at that path. The requested selection does not resolve this profile; a launch that selects it refuses it as unsafe, not as missing."),
                action: Some("Correct the credential path of the profile so that every component before the file name is a directory, or remove the profile. Rerun with --show-paths to see the recorded path."),
            },
            TextVariant {
                name: "path",
                message: Some("The credential file of a declared profile is refused by its reader for its path: a directory that leads to the file can be written by group or others, belongs to another account than root or the control account, is a symbolic link, cannot be opened, or is on an unsupported file system; on macOS, such a directory can also carry an extended access entry that permits a modification. The requested selection does not resolve this profile; a launch that selects it refuses. The verdict is that of the product walk, which this tool asked without opening the file."),
                action: Some("Keep the file below directories that only root or the control account can write, without a symbolic link on the way, or remove the profile. Rerun with --show-paths to see the recorded path."),
            },
        ],
        commands: &[product(&["run"], &[]), tool(&["--show-paths"])],
    }
    ProviderCredentialContentNotChecked = "provider.credential_content_not_checked" {
        axis: Providers, status: NotChecked, basis: Observed,
        consumer: "agentsandbox run",
        message: "This tool opens no file that the profile file names as a provider credential, so the content of the credentials and the validity of the account are not checked.",
        action: "A launch validates the selected credential before authorization.",
        requires: "agentsandbox run validates the selected credential before authorization",
        variants: &[TextVariant {
            name: "after_read",
            message: Some("Apart from a file that reference.file_read_before_it_was_named reports, this tool opened no file that the profile file names as a provider credential. The content of the credentials and the validity of the account are not checked."),
            action: None,
        }],
        commands: &[product(&["run"], &[])],
    }
    ProviderManualLaunchPathNotChecked = "provider.manual_launch_path_not_checked" {
        axis: Providers, status: NotChecked, basis: Observed,
        consumer: "agentsandbox run",
        message: "The manual launch path, with the options --provider and --secret-env-file and an explicit command, does not use the profile file and is not checked.",
        action: "A launch applies its own rules to that path, including the rule on env file names inside the project.",
        requires: "agentsandbox run",
        commands: &[product(&["run"], &["--provider", "--secret-env-file"])],
    }
    ProviderLaunchRequiresObserveAndBridge = "provider.launch_requires_observe_and_bridge" {
        axis: Providers, status: Info, basis: Inferred,
        consumer: "agentsandbox run",
        message: "A managed launch with provider credentials requires the options --profile observe and --network bridge; it changes neither by itself.",
        action: "Pass both options to agentsandbox run for a managed client.",
        commands: &[product(&["run"], &["--profile", "--network"])],
    }
    ProviderSelectionResolved = "provider.selection_resolved" {
        axis: Providers, status: Ok, basis: Observed,
        consumer: "agentsandbox run",
        message: "The requested selection resolves to one profile. The choice is made by the product function that a managed launch uses, for the selection options given to this tool; that function reads no credential file. The product rule is: an explicit profile; then the default profile among the candidates; then a single candidate; then auto_priority; under automatic authentication mode, subscription candidates exclude API candidates.",
        action: "No action is needed for the selection. Give this tool the options --client, --auth-mode, --client-profile and --model exactly as agentsandbox run receives them; an option left out is resolved automatically.",
        commands: &[
            product(&["run"], &["--client", "--auth-mode", "--client-profile", "--model"]),
            tool(&["--client", "--auth-mode", "--client-profile", "--model"]),
        ],
    }
    ProviderSelectionAmbiguous = "provider.selection_ambiguous" {
        axis: Providers, status: Error, basis: Observed,
        consumer: "agentsandbox run",
        message: "The product refuses the requested selection as ambiguous: several profiles match and neither default_profile nor auto_priority decides between them, or the requested client or authentication mode contradicts the requested profile. A managed launch with this selection refuses. The reason is a closed code.",
        action: "Request one profile with --client-profile ID, narrow the request with --client or --auth-mode, or declare default_profile or auto_priority in the profile file. Give this tool the same selection options as agentsandbox run.",
        commands: &[
            product(&["run"], &["--client", "--auth-mode", "--client-profile"]),
            tool(&["--client", "--auth-mode", "--client-profile"]),
        ],
    }
    ProviderSelectionNoProfile = "provider.selection_no_profile" {
        axis: Providers, status: Error, basis: Observed,
        consumer: "agentsandbox run",
        message: "No declared profile answers the requested selection: the requested profile is not declared, or no profile matches the requested client and authentication mode. A managed launch with this selection refuses, and no profile of another authentication mode is used in its place. The reason is a closed code.",
        action: "Request a declared profile, or declare a profile for the requested client and authentication mode. Give this tool the same selection options as agentsandbox run.",
        commands: &[product(&["run"], &[])],
    }
    ProviderSelectionInvalid = "provider.selection_invalid" {
        axis: Providers, status: Error, basis: Observed,
        consumer: "agentsandbox run",
        message: "A selection option has a value that the product validator refuses. A profile identifier is 1 to 80 ASCII letters, digits, hyphens or underscores. A model identifier is 1 to 128 ASCII letters, digits, underscores, dots, slashes, colons or hyphens, and does not start with a hyphen. The reason is a closed code.",
        action: "Correct the value given with --client-profile or --model.",
        commands: &[
            product(&["run"], &[]),
            tool(&["--client-profile", "--model"]),
        ],
    }
    ProviderSelectionNotResolved = "provider.selection_not_resolved" {
        axis: Providers, status: NotChecked, basis: Observed,
        consumer: "agentsandbox run",
        message: "Selection options were given, but no provider profile file was loaded in this run, so no selection was resolved.",
        action: "Correct what this axis reports for the profile file, or pass --config-dir DIR or --providers-file FILE, then give the same selection options again.",
        requires: "a provider profile file that the product loader accepts",
        commands: &[
            product(&["run"], &[]),
            tool(&["--config-dir", "--providers-file"]),
        ],
    }
    ProviderSubscriptionNeverFallsBackToApi = "provider.subscription_never_falls_back_to_api" {
        axis: Providers, status: Info, basis: Inferred,
        consumer: "agentsandbox run",
        message: "The requested selection resolves to a subscription profile whose credential file is absent or unsafe, so a managed launch with this selection refuses. The profile file also declares API profiles: none of them was tried. The product never replaces a subscription by an API profile.",
        action: "Sign in for the resolved subscription profile, or correct its credential reference, as the finding on that profile says. No API profile is used in its place. Under automatic authentication mode an API profile can be resolved when no subscription profile matches the requested client; a request made with --auth-mode subscription never resolves to an API profile.",
        variants: &[
            TextVariant {
                name: "request_refused",
                message: Some("A subscription was requested with --auth-mode subscription and the product refuses this selection. The profile file also declares API profiles: none of them was tried. The product never replaces a requested subscription by an API profile."),
                action: Some("Correct the request or the profile file as the selection finding of this report says. No API profile is used in place of a requested subscription. Under automatic authentication mode an API profile can be resolved when no subscription profile matches the requested client; a request made with --auth-mode subscription never resolves to an API profile."),
            },
            TextVariant {
                name: "no_subscription_profile",
                message: Some("A subscription was requested with --auth-mode subscription, and the profile file declares no subscription profile: a managed launch with this selection refuses. The API profiles that the file declares were not tried. The product never replaces a requested subscription by an API profile."),
                action: Some("Declare a subscription profile and sign in for it, as the provider guide describes. No API profile is used in place of a requested subscription. Without --auth-mode subscription, the automatic authentication mode can resolve an API profile of this file, and its usage is then billed by the provider."),
            },
        ],
        commands: &[
            product(&["run"], &["--auth-mode"]),
            tool(&["--auth-mode"]),
        ],
    }
    ProviderApiBillingNotice = "provider.api_billing_notice" {
        axis: Providers, status: Warning, basis: Inferred,
        consumer: "agentsandbox run",
        message: "The requested selection resolves to a profile in API authentication mode, so usage is billed by the provider. A launch prints the same notice before it starts the worker.",
        action: "No action is needed when billed API use is intended. A request made with --auth-mode subscription never resolves to an API profile.",
        commands: &[
            product(&["run"], &[]),
            tool(&["--auth-mode"]),
        ],
    }

    // ---- inference --------------------------------------------------------
    InferenceDecisionEngineSelectionNotPersisted = "inference.decision_engine_selection_not_persisted" {
        axis: Inference, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The decision engine is selected by gateway options and variables that are not stored in the installation: the selection is not checked. For a backend that calls an engine, the gateway requires its URL and its model, and a key file for the TypeSafe backend; it refuses an incomplete or mixed set of decision-engine options.",
        action: "Read the gateway launcher: the option --jev-backend, then the variable named JEV_BACKEND, then a legacy URL option; with none of them no decision engine is used.",
        requires: "the command line or the Compose file of the launcher",
        commands: &[product(&["gateway"], &["--jev-backend"])],
    }
    InferenceDecisionTransportValid = "inference.decision_transport_valid" {
        axis: Inference, status: Ok, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The given decision-engine transport is accepted by the product validator: URL form, local HTTP permission and CA bundle. The engine was not contacted.",
        action: "No action is needed for the transport. The gateway also checks its whole set of decision-engine options: the backend it selects, its model, and a key file for the TypeSafe backend. It refuses an incomplete or mixed set, which this tool does not check.",
        commands: &[product(&["gateway"], &[])],
    }
    InferenceDecisionTransportInputsWithoutUrl = "inference.decision_transport_inputs_without_url" {
        axis: Inference, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "A local HTTP permission or a CA bundle for the decision engine was given without a decision-engine URL: no transport was judged, and a CA bundle is judged by its grammar alone. A gateway started with one of these options and without a decision-engine URL, by option or by variable, refuses to start; with a URL, it refuses the local HTTP permission together with a CA bundle.",
        action: "Give the URL that the gateway launcher uses, with --jev-url URL or --jev-local-url URL.",
        requires: "--jev-url URL or --jev-local-url URL",
        commands: &[product(&["gateway"], &[]), tool(&["--jev-url", "--jev-local-url"])],
    }
    InferenceDecisionTransportNotChecked = "inference.decision_transport_not_checked" {
        axis: Inference, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The given decision-engine transport was not judged. The CA bundle given with it could not be used, and the gateway builds its transport with that bundle. The URL alone is not refused by the product validator.",
        action: "Correct what the trust axis reports for the CA bundle, then run this tool again with the same options.",
        requires: "a CA bundle that the product accepts, given with --jev-ca-file FILE",
        commands: &[product(&["gateway"], &[]), tool(&["--jev-ca-file"])],
    }
    InferenceLocalModelBackendNotAvailable = "inference.local_model_backend_not_available" {
        axis: Inference, status: NotAvailable, basis: Observed,
        message: "This build has no offline inspection of a local model backend: image, weights, memory budget and readiness are not examined.",
        action: "Nothing in this build covers it offline. The local-AI guide describes how to build and verify the backend.",
    }
    InferenceModelTlsOutputsNotAvailable = "inference.model_tls_outputs_not_available" {
        axis: Inference, status: NotAvailable, basis: Observed,
        message: "This build has no reader for the outputs of model TLS provisioning.",
        action: "Nothing in this build covers it offline.",
    }

    // ---- trust ------------------------------------------------------------
    TrustOidcCaBundleAcceptedByGrammar = "trust.oidc_ca_bundle_accepted_by_grammar" {
        axis: Trust, status: Ok, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The CA bundle of the OIDC file is accepted by the bundle grammar of the product and by the offline client build: certificate sections only, at most 8 certificates and 65536 bytes.",
        action: "No action is needed for the grammar. Validity dates are listed separately.",
        commands: &[product(&["gateway"], &[])],
    }
    TrustDecisionEngineCaAcceptedByGrammar = "trust.decision_engine_ca_accepted_by_grammar" {
        axis: Trust, status: Ok, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The CA bundle given for the decision engine is accepted by the bundle grammar of the product: certificate sections only, at most 8 certificates and 65536 bytes.",
        action: "No action is needed for the grammar. Validity dates are listed separately.",
        commands: &[product(&["gateway"], &[])],
    }
    TrustGatewayCaAcceptedByGrammar = "trust.gateway_ca_accepted_by_grammar" {
        axis: Trust, status: Ok, basis: Observed,
        message: "The CA bundle given for the clients of an HTTPS gateway is accepted by the bundle grammar of the product: certificate sections only, at most 8 certificates and 65536 bytes.",
        action: "No action is needed for the grammar. Validity dates are listed separately.",
    }
    TrustCaBundleInvalid = "trust.ca_bundle_invalid" {
        axis: Trust, status: Error, basis: Observed,
        message: "A CA bundle is refused by the bundle grammar of the product: certificate sections only, ASCII, at most 8 certificates and 65536 bytes. Its content is not printed.",
        action: "Provide a PEM bundle that holds only public certificates.",
    }
    TrustCaIncompatibleWithTransport = "trust.ca_incompatible_with_transport" {
        axis: Trust, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The CA bundle and the gateway block of the OIDC file are each valid alone but refused together: a CA bundle requires the https_only transport.",
        action: "Use the https_only transport with this CA bundle. The isolated demonstration transport accepts no CA bundle.",
        commands: &[product(&["gateway"], &[])],
    }
    TrustCaUnavailableOrNotPrivate = "trust.ca_unavailable_or_not_private" {
        axis: Trust, status: Error, basis: Observed,
        message: "A CA bundle is refused by the file rule of its consumer, or its metadata cannot be read. The file was not read.",
        action: "Give the bundle the posture that its consumer requires: a private file (mode 0600, single link) for the reference of the OIDC file; a regular single-link file that other accounts cannot write for a bundle given by option.",
        variants: &[TextVariant {
            name: "absent",
            message: Some("A CA bundle does not exist at its recorded or given path. Nothing was read."),
            action: Some("Restore the bundle at that path, or correct the reference or the option that names it."),
        }],
    }
    TrustCaReferenceIsACredentialReference = "trust.ca_reference_is_a_credential_reference" {
        axis: Trust, status: Error, basis: Observed,
        message: "A CA bundle reference points at a file that this installation uses as a key file, a setup record or a store, or below a directory that holds only such files. The file was not opened.",
        action: "Point the CA reference at a PEM bundle of public certificates.",
    }
    TrustDecisionTransportInvalid = "trust.decision_transport_invalid" {
        axis: Trust, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The given decision-engine transport is refused by the product validator.",
        action: "Use an HTTPS URL without user information, query or fragment. The explicit local HTTP permission and a CA bundle are refused together, whatever the URL: with a CA bundle, start the gateway without that permission. Plain HTTP is accepted only for a loopback address, with that permission and without a CA bundle.",
        variants: &[TextVariant {
            name: "without_bundle",
            message: Some("The given decision-engine URL is refused by the product validator even without the CA bundle, which could not be used."),
            action: None,
        }],
        commands: &[product(&["gateway"], &[])],
    }
    TrustCertificateValidityPeriodNotAvailable = "trust.certificate_validity_period_not_available" {
        axis: Trust, status: NotAvailable, basis: Observed,
        message: "The bundle grammar does not evaluate validity dates: this build does not detect an expired certificate.",
        action: "Nothing in this build covers it. Check the dates with the tooling of your certificate authority.",
    }

    // ---- engine -----------------------------------------------------------
    EngineProgramNotInspected = "engine.program_not_inspected" {
        axis: Engine, status: NotChecked, basis: Observed,
        message: "No engine was named, so no engine program was inspected.",
        action: "Pass --runtime docker or --runtime podman.",
        requires: "--runtime docker or --runtime podman",
        commands: &[tool(&["--runtime"])],
    }
    EngineStateNotChecked = "engine.state_not_checked" {
        axis: Engine, status: NotChecked, basis: Observed,
        message: "This tool never executes the engine: whether it runs, its version and its rootless mode are not checked.",
        action: "Run agentsandbox doctor --runtime docker or agentsandbox doctor --runtime podman; that command executes the engine.",
        requires: "agentsandbox doctor --runtime docker or agentsandbox doctor --runtime podman",
        commands: &[product(&["doctor"], &["--runtime"])],
    }
    EngineProgramApproved = "engine.program_approved" {
        axis: Engine, status: Ok, basis: Observed,
        message: "The engine program passes the approval rule that the product applies before any engine call. It was hashed, not executed.",
        action: "No action is needed. The engine state is listed as not checked.",
    }
    EngineProgramAbsentFromFixedEntries = "engine.program_absent_from_fixed_entries" {
        axis: Engine, status: Error, basis: Observed,
        message: "No engine program of this kind exists at the fixed system locations the product looks at. The PATH variable is never searched.",
        action: "Install the engine, or pass its absolute path with --runtime-program ABS.",
        commands: &[tool(&["--runtime-program"])],
    }
    EngineProgramRefused = "engine.program_refused" {
        axis: Engine, status: Error, basis: Observed,
        message: "The engine program is refused by the approval rule of the product. The reason is a closed code.",
        action: "Use an engine program and helper directories that other accounts cannot rewrite, and run this tool as the control account.",
        variants: &[
            TextVariant {
                name: "absent",
                message: Some("Nothing exists at the given engine program path, so the approval rule of the product refuses it. The reason is a closed code."),
                action: Some("Give the absolute path of an existing engine program with --runtime-program ABS, or leave that option out to use the fixed system locations."),
            },
            TextVariant {
                name: "relative",
                message: Some("The given engine program is not an absolute path. The product approves the path exactly as it is written and refuses a relative one. The reason is a closed code."),
                action: Some("Give the absolute path of the engine program with --runtime-program ABS."),
            },
            TextVariant {
                name: "path_form",
                message: Some("The approval refuses the form of a path: the engine program, or the home directory of the control account, is not an absolute UTF-8 path without parent traversal and without control characters. The reason is a closed code."),
                action: Some("Give the engine program as an absolute path without parent traversal with --runtime-program ABS, and check that the control account has an absolute home directory in the password database."),
            },
            TextVariant {
                name: "unresolved",
                message: Some("The engine program path cannot be resolved to a file: a link on the way leads nowhere, or a directory on the way cannot be traversed. The reason is a closed code."),
                action: Some("Give the absolute path of the engine program itself with --runtime-program ABS."),
            },
            TextVariant {
                name: "directory_unavailable",
                message: Some("The engine program path resolves to the root of the file system, which is no program in a directory. The reason is a closed code."),
                action: Some("Give the absolute path of the engine program file with --runtime-program ABS."),
            },
            TextVariant {
                name: "directory_not_trusted",
                message: Some("The directory that holds the engine program, once its links are resolved, belongs to another account than root or the control account, or its group or other accounts can write it. The reason is a closed code."),
                action: Some("Use an engine program installed in a directory that only root or the control account can write. Neither this tool nor the product changes an existing directory."),
            },
            TextVariant {
                name: "program_not_trusted",
                message: Some("The engine program, once its links are resolved, is not a regular executable file owned by root or the control account, or its group or other accounts can write it. The reason is a closed code."),
                action: Some("Give a regular executable file owned by root or the control account, without group or other write permission, with --runtime-program ABS."),
            },
            TextVariant {
                name: "size_bound",
                message: Some("The engine program is larger than 512 MiB: the product approves no program above that size. The reason is a closed code."),
                action: Some("Give the engine program itself with --runtime-program ABS, not an archive or an image that contains it."),
            },
            TextVariant {
                name: "link_outside_approved_roots",
                message: Some("The engine found at a fixed system location is a link to a program outside the installation locations that the product approves without an explicit path. The reason is a closed code."),
                action: Some("Pass the absolute path of the engine program with --runtime-program ABS: the product then approves that explicit path under the same rule."),
            },
            TextVariant {
                name: "filesystem_unsupported",
                message: Some("The engine program, or the directory that holds it, is on a file system that the product does not support for a program it approves: a network or FUSE file system, or a volume that ignores ownership. The reason is a closed code."),
                action: Some("Install the engine on a supported local file system."),
            },
            TextVariant {
                name: "directory_not_pinned",
                message: Some("The directory that holds the engine program cannot be opened by the account running this tool without following a link: a directory on the way grants it no access, or was replaced by a link. The text of the system error is not printed. The reason is a closed code."),
                action: Some("Run this tool as the control account, and check that this account can open each directory that leads to the engine program."),
            },
            TextVariant {
                name: "program_not_opened",
                message: Some("The engine program cannot be opened for reading by the account running this tool, so the product cannot hash it. The text of the system error is not printed. The reason is a closed code."),
                action: Some("Run this tool as the control account, and check that this account can read the engine program."),
            },
            TextVariant {
                name: "account_home",
                message: Some("The approval could not use the home directory that the password database gives for the account running this tool: it gives none, or its path is not UTF-8. The reason is a closed code."),
                action: Some("Run this tool as the control account, whose entry in the password database names a home directory."),
            },
            TextVariant {
                name: "helper_directories",
                message: Some("A system directory of the helper path that the product gives to the engine, /usr/bin, /bin, /usr/sbin or /sbin, is not owned by root or can be written by group or others. The reason is a closed code."),
                action: Some("Restore root ownership, without group or other write permission, on those system directories."),
            },
            TextVariant {
                name: "runtime_directory",
                message: Some("The runtime directory of the account running this tool, /run/user/UID, exists and is not a directory of that account closed to group and others. The product gives this refusal no reason of its own: the reason is unclassified."),
                action: Some("Restore the ownership of that directory to the account, with mode 0700, as the login service creates it."),
            },
            TextVariant {
                name: "extended_access_entry",
                message: Some("The engine program, its directory or a system directory of the helper path carries an extended access entry, which the product refuses. The product gives this refusal no reason of its own: the reason is unclassified."),
                action: Some("Use an engine installation without extended access entries on the program and on its directory."),
            },
        ],
        commands: &[tool(&["--runtime-program"])],
    }
    EngineProgramIsACredentialReference = "engine.program_is_a_credential_reference" {
        axis: Engine, status: Error, basis: Observed,
        message: "The given engine program is a file that this installation uses as a key file, a setup record or a store, or lies below a directory that holds only such files, as written or once its links are resolved. It was not opened. No approval was attempted and no digest was computed.",
        action: "Give the absolute path of the engine program itself with --runtime-program ABS.",
        commands: &[tool(&["--runtime-program"])],
    }

    // ---- data -------------------------------------------------------------
    DataOfflineInspectionNotAvailable = "data.offline_inspection_not_available" {
        axis: Data, status: NotAvailable, basis: Observed,
        message: "This diagnostic never opens SQLite connections to tenant stores or authentication audits: schema, tenant binding, audit format and integrity are not examined here.",
        action: "For tenant audit evidence, use the signed-checkpoint guide as a separate explicit operation. A gateway local-chain check and independently anchored rollback verification are different operations.",
        variants: &[
            TextVariant {
                name: "historical",
                message: Some("This diagnostic never opens SQLite connections to tenant stores or authentication audits: schema, tenant binding, audit format and integrity are not examined here. The online command agentsandbox verify-audit --config-dir DIR --gateway URL asks the running gateway for its local hash-chain check and reads the viewer file."),
                action: None,
            },
            TextVariant {
                name: "prepared",
                message: Some("This diagnostic never opens SQLite connections to tenant stores or authentication audits: schema, tenant binding, audit format and integrity are not examined here. The gateway validates its stores when it starts. For a tenant local-chain check, agentsandbox verify-audit --viewer-token-file FILE --gateway URL reads an explicit viewer file; it does not establish independent rollback continuity."),
                action: None,
            },
        ],
        commands: &[product(&["verify-audit"], &["--config-dir", "--gateway", "--viewer-token-file"])],
    }
    DataSignedCheckpointNotChecked = "data.signed_checkpoint_not_checked" {
        axis: Data, status: NotChecked, basis: Observed,
        consumer: "ai-guard-audit-checkpoint",
        message: "Signed tenant checkpoints and independent rollback continuity were not verified by this diagnostic.",
        action: "Use ai-guard-audit-checkpoint verify as a separate, deliberate offline operation with tenant checkpoint evidence and an independently held trust anchor. Follow the signed-checkpoint guide for optional database comparison and its sidecar effects.",
        requires: "explicit ai-guard-audit-checkpoint verify with tenant checkpoint evidence and an independently held trust anchor",
    }
    DataLocationNotPersisted = "data.location_not_persisted" {
        axis: Data, status: NotChecked, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The store path of a historical config directory is a gateway option that is not stored in the installation: no store was located.",
        action: "Read the gateway launcher: the option --data, or the Compose volume.",
        requires: "the command line or the Compose file of the launcher",
        commands: &[product(&["gateway"], &["--data"])],
    }
    DataStoreAbsent = "data.store_absent" {
        axis: Data, status: Warning, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "No store exists at the recorded path of a tenant. The gateway creates a new empty store at this path when it starts.",
        action: "If a store was expected here, do not start the gateway; locate the existing file first.",
        commands: &[product(&["gateway"], &[])],
    }
    DataAuthenticationAuditAbsent = "data.authentication_audit_absent" {
        axis: Data, status: Warning, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "No authentication audit exists at the recorded path. The gateway refuses to start, unless it is started once with --initialize-oidc-audit, which creates an exclusively new audit.",
        action: "If an audit was expected here, locate the existing file first. Use the gateway option --initialize-oidc-audit only for a new installation, never to replace an existing audit.",
        commands: &[product(&["gateway"], &["--initialize-oidc-audit"])],
    }
    DataAuthenticationAuditMissingFromPreparedInstallation = "data.authentication_audit_missing_from_prepared_installation" {
        axis: Data, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The authentication audit that setup prepare created for this installation does not exist at its recorded path. A gateway started as the SSO guide documents, without --initialize-oidc-audit, refuses to start.",
        action: "Locate the original audit and restore it at its recorded path, with the consumers stopped. Never initialise a replacement audit over a prepared installation.",
        commands: &[product(&["gateway"], &["--initialize-oidc-audit"])],
    }
    DataStorePostureRefused = "data.store_posture_refused" {
        axis: Data, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "A store is refused by the private-entry rule of the gateway: a regular file owned by the control account, mode 0600, one link.",
        action: "Restore that posture on the listed store.",
        variants: &[
            TextVariant {
                name: "path",
                message: Some("A recorded store path is refused by the gateway: it contains a parent traversal."),
                action: Some("Record the store path without parent traversal."),
            },
            TextVariant {
                name: "name",
                message: Some("A recorded store path is refused by the gateway for its file name: the name is not ASCII, or it is a name that the product or SQLite reserves, starting with .ai-guard-sqlite- or ending with -wal, -shm or -journal, whatever the case. No store is created at such a path."),
                action: Some("Record a store whose file name is ASCII and is none of those reserved names. Preserve existing data: giving a store another name is a controlled offline operation."),
            },
        ],
        commands: &[product(&["gateway"], &[])],
    }
    DataParentDirectoryNotPrivate = "data.parent_directory_not_private" {
        axis: Data, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "The directory that holds a store is refused by the private-directory rule of the gateway: it must be owned by the control account, have mode 0700, and sit below directories that other accounts cannot rewrite, without a symbolic link in its path, on a supported local file system.",
        action: "Use a directory that meets this rule for the listed stores. The product never moves an existing file.",
        commands: &[product(&["gateway"], &[])],
    }
    DataOrphanSidecars = "data.orphan_sidecars" {
        axis: Data, status: Error, basis: Observed,
        consumer: "agentsandbox gateway",
        message: "SQLite sidecar files exist without their store. The gateway refuses to open or create a store beside them.",
        action: "Preserve the sidecars and restore the intended store before any start.",
        commands: &[product(&["gateway"], &[])],
    }
    DataSidecarsPresent = "data.sidecars_present" {
        axis: Data, status: Info, basis: Observed,
        message: "SQLite sidecar files exist beside a store: it is in use, or it was not closed cleanly.",
        action: "No action is needed. Do not remove them: they belong to the store.",
    }

    // ---- output -----------------------------------------------------------
    OutputValueMasked = "output.value_masked" {
        axis: Output, status: Warning, basis: Observed,
        message: "A value looks like a credential to the output filter and is shown as a placeholder. Identifiers are numbered in sorted order.",
        action: "If the value is a legitimate identifier or path, its placeholder is stable between runs. Rename the value to remove the ambiguity.",
    }
    OutputDetailOmitted = "output.detail_omitted" {
        axis: Output, status: Info, basis: Observed,
        message: "The report exceeded its size bound, so its detail lists were emptied. Every item was still evaluated and every total is exact.",
        action: "Inspect one target at a time to see the omitted entries.",
    }

    // ---- limits of the build ----------------------------------------------
    ProviderQualifiedVersionsNotAvailable = "provider.qualified_versions_not_available" {
        axis: Providers, status: NotAvailable, basis: Observed,
        limit: true,
        consumer: "agentsandbox run",
        message: "This build contains no list of qualified client versions.",
        action: "Record the client versions you build into the worker image; this tool cannot confirm them.",
        commands: &[product(&["run"], &[])],
    }
    ProviderClientPresenceNotAvailable = "provider.client_presence_not_available" {
        axis: Providers, status: NotAvailable, basis: Observed,
        limit: true,
        consumer: "agentsandbox run",
        message: "Whether Codex CLI or Claude Code is present is probed only inside the worker when it starts. This build has no check on the control side.",
        action: "Nothing in this build covers it offline. A launch reports an absent client from the worker.",
        commands: &[product(&["run"], &[])],
    }
    ProviderClientVersionNotAvailable = "provider.client_version_not_available" {
        axis: Providers, status: NotAvailable, basis: Observed,
        limit: true,
        consumer: "agentsandbox run",
        message: "The version of a client is probed only inside the worker when it starts. This build has no check on the control side.",
        action: "Nothing in this build covers it offline. A launch reports an incompatible version from the worker.",
        commands: &[product(&["run"], &[])],
    }
    ProviderAccountValidityNotAvailable = "provider.account_validity_not_available" {
        axis: Providers, status: NotAvailable, basis: Observed,
        limit: true,
        message: "Whether a provider account or a subscription is valid can only be established by the provider. This tool makes no provider call.",
        action: "Nothing in this build covers it. A launch is the first use of the account.",
    }
    IdentityArbitraryOidcProviderNotQualified = "identity.arbitrary_oidc_provider_not_qualified" {
        axis: Identity, status: NotAvailable, basis: Observed,
        limit: true,
        message: "The product accepts one exact signed OIDC profile. Compatibility with a given identity provider is not presumed and is not qualified by this tool.",
        action: "Follow the SSO guide and qualify your identity provider with a real sign-in.",
    }
    InferenceInternalProfileNotAvailable = "inference.internal_profile_not_available" {
        axis: Inference, status: NotAvailable, basis: Observed,
        limit: true,
        message: "This build has no managed internal-inference profile for Codex CLI or Claude Code: a provider profile carries no URL, no format and no CA field.",
        action: "Nothing in this build covers it. The enterprise guide states what is delivered.",
    }
    TopologyKubernetesLaunchNotAvailable = "topology.kubernetes_launch_not_available" {
        axis: Topology, status: NotAvailable, basis: Observed,
        limit: true,
        message: "Worker launch through the Kubernetes adapter is not available in this build; its diagnostics do not enable admission.",
        action: "Use Docker or Podman for workers. To inspect an authority without admitting workers, use agentsandbox kubernetes doctor.",
        commands: &[product(&["kubernetes", "doctor"], &[])],
    }
    LayoutSiblingStagingNotInspected = "layout.sibling_staging_not_inspected" {
        axis: Layout, status: NotAvailable, basis: Observed,
        limit: true,
        message: "This tool looks only inside the directories it is given. A setup staging directory left beside a published installation is not searched for.",
        action: "If a preparation was interrupted, pass the retained staging directory that setup prepare printed as --installation-dir DIR.",
        commands: &[tool(&["--installation-dir"])],
    }
    ToolLinuxMacosPosturePolicyOnly = "tool.linux_macos_posture_policy_only" {
        axis: Layout, status: NotAvailable, basis: Observed,
        limit: true,
        message: "Private-directory and private-file checks follow the product policy for local Linux and macOS file systems. A network or FUSE file system is refused by that policy, not examined.",
        action: "Keep the installation on a supported local file system.",
    }
}
