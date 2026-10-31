Correction de métadonnées de preuve

La capture originale `/private/tmp/ai-guard-final-canaries.kOMSpQ/snapshot/SHA256SUMS` est conservée intacte. Son manifeste contenait une ligne pour `./SHA256SUMS` lui-même : il ne pouvait donc pas fournir un hash auto-référent vérifiable et les 150 lignes ne correspondaient pas à 150 fichiers du gel.

Cette preuve corrigée manifeste exactement les 149 fichiers du gel final: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, tous les fichiers de `src/` et tous les fichiers de `tests/`. Le manifeste se trouve hors du répertoire `sources/` et hors de l'archive. Les 149 entrées ont été vérifiées contre le snapshot original et le checkout courant. L'archive est extraite et les mêmes 149 checksums passent.

Aucune source, aucun test, aucune exécution, aucun binaire ni aucun artefact initial n'a été modifié pour cette correction de provenance.
