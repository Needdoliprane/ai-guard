# Codex TUI two-turn qualification attempt

## Result

**FAIL — two-turn criterion not met.** One fresh Codex TUI session was admitted with the requested `gpt-6-luna` model selection and the private API envfile injected through the broker. The first requested filesystem effect is present in the retained workspace, but no first-turn receipt or clearly observable idle composer was available. I did not send the second user message or `/exit`.

The broker reached the authorized 300-second duration limit. The runner result was `exit_code=137`, `timed_out=true`, `changed_files=1`, `output_captured=false`; cleanup removed the container successfully, and the transport stop reason was `authorized worker duration elapsed`. The worker's final registry state was `terminal`, receipt phase `initial`, quota no longer held, cleanup marked removed, and no cleanup error. This does not establish successful TUI response/receipt delivery.

## Run identity and bounded command

- Broker binary: `/tmp/ai-guard-interactive-qualified.hO9L2x/agentsandbox`, SHA-256 `7a86a4ffb494c706de197931ecfbd00da20be23263ce0d64bd60b5e692e1e3ed`.
- Worker image: `agentsandbox-worker:qa-codex-0.165.0-20261030`, image ID `sha256:42cfc041ee5daa0128da53da45c580e66aa1c17839b5013f053f7221d7179cab`; CLI version was Codex `0.165.0`.
- Session `59ab90c8-e4a6-4d54-9320-fdeab852e871`; worker `9f49d373-e044-4262-b1c0-1d312ce93166`; attempt `63d89cef-532e-4561-b11e-f820b2e24afc`; container `bce01a501318776dc23f7f73e7aa0e08cdbbb7c1befbe2f30c16689789a2d2b1`.
- Private gateway: `http://127.0.0.1:48791`; config and fixture are under `/tmp/ai-guard-codex-tui-two-turns.AhbISg/`. The gateway was stopped cleanly after final read-only checks.
- Launch used `--runtime docker --runtime-program /opt/homebrew/bin/docker --runtime-endpoint unix:///Users/louis/.docker/run/docker.sock --allow-rootful-runtime --profile observe --network bridge --provider codex --interactive --timeout 300`, with `--secret-env-file` referencing the private `CODEX_API_KEY` envfile. The key value was never read into tool output or written to this report.
- TUI args: `codex --no-alt-screen --strict-config --model gpt-6-luna --sandbox danger-full-access --ask-for-approval never -c 'model_provider="ai_guard_api"' -c 'model_providers.ai_guard_api.name="ai-guard OpenAI API"' -c 'model_providers.ai_guard_api.base_url="https://api.openai.com/v1"' -c 'model_providers.ai_guard_api.env_key="CODEX_API_KEY"' -c 'model_providers.ai_guard_api.requires_openai_auth=false' -c 'model_providers.ai_guard_api.wire_api="responses"' '<first-turn instruction>'`. No `codex exec`, reviewer model, automatic retry, or second session was used.

## Read-only evidence

- Initial fixture hashes: `FIRST_TURN.txt` `9067625e27c6408d77dffa99b7bde5fdff7330e0001ce31a36fe3c85f7369d46`; `SECOND_TURN.txt` `a062260d0893dba98f28f2f88c2ab9964567df787eb994eaba270891d207797f`. Final project hashes match these values.
- Export to a new destination succeeded: 3 verified files in `/tmp/ai-guard-codex-tui-two-turns.AhbISg/export-one-turn/`.
- Exported `result.txt` is exactly 19 bytes, one line, `turn one: complete\n`; SHA-256 `8142525ba5170feffdc28c24f22b31f8b911ea952142db58ca55a3371c910283`. This proves the first requested file effect, but not a visible assistant receipt or second turn.
- Audit verification: valid, 26 events, head `bfe6f01eeca1d04a42799d3a90480325a9578fe06b7eff2c64a2d398e9b7f875`.
- Usage query: exactly 1 allocation matches the worker and attempt; `billable=false`, method version 1, runtime `docker`, provenance `observed`. Final quota state is released (`quota_held=false`).
- TUI polling showed a footer warning count of 2 once; warning details and the final composer state could not be established from the bounded output. No raw TUI transcript was retained.

## Scope and limitations

Only the first positional message was sent. There was no second prompt, `/exit`, or model-serving attestation; the TUI did not expose a confirmed idle/receipt point before the duration bound. The file effect is real and exported, but the requested two-turn behavior remains unqualified. The runner reported `output_captured=false`, so the receipt could not be validated from captured output. No provider retry, additional model session, Docker privilege change, or product-code edit was made.

A bounded in-memory literal-key scan covered this report, both fixture instruction files, and the exported `result.txt`: 4 files scanned, 0 files/hits. The scanner emitted only counts and retained no key value.

### Réserve sur l’observabilité de la sortie interactive

Une réponse d’outil antérieure à la reprise après compaction a indiqué que la sortie dépassait le contexte disponible et était tronquée; son contenu n’est plus accessible pour vérification et a pu contenir un reçu. La conclusion demeure **qualification deux tours non démontrée faute de preuve de visibilité/idle**. Ce relevé ne prouve pas une défaillance du broker, et output_captured=false ne permet pas d’inférer que Codex n’a pas produit de reçu.
