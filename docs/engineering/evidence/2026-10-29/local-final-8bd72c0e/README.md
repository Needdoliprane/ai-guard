# Preuves finales locales — 29 octobre 2026

Copies de journaux et rapports existants de l'orchestrateur et des reviewers, sans réexécution des tests ni action runtime pendant la consolidation. Aucun fichier historique du dossier parent n'est écrasé. Le statut de qualification est dans [VALIDATION.md](../../../VALIDATION.md), l'[état courant](../../../CURRENT_STATE.md) et le [plan](../../../IMPLEMENTATION_PLAN.md).

Périmètre : macOS 27.2 ARM64, Rust 1.99.1, Docker Desktop 4.97.0 (244584)/Engine 29.9.2, CLI 29.2.1, exception rootful explicite. La pile QA `ai-guard-qa-7dce8f12` utilise l'image contrôle `agentsandbox-control:qa-20261029-8bd72c0e`, ID local `sha256:70b34787cc12aa59e6558c1e391329bffa0acc0c6bc83363737a46589a02414e`. Bindings nommés : gateway `127.0.0.1:48787`, dashboard `127.0.0.1:48400`. Ce dossier ne qualifie ni rootless, Linux x86_64, Podman, Kubernetes, trois workers, charge 100, fournisseurs, ni une release.

## Résultats et fichiers conservés

| Preuve | Résultat et portée |
|---|---|
| [Clippy](rust-clippy-final.log), [tests](rust-tests-final.log), [release](rust-release-final.log) | 139 tests logiciels PASS, zéro échec, deux réels ignorés dans cette commande globale. Build release fini ; aucun PASS runtime déduit. |
| [Clippy après delta smoke](smoke-delta-clippy-final.log), [fmt après delta smoke](smoke-delta-fmt-final.log) | Succès observés par l'orchestrateur ; la sortie fmt est vide. |
| [Confinement final réel](runtime-confinement-final.log), [rapport](runtime-confinement-report.md), [revue](review-runtime-confinement.md) | 1 test réel PASS sur Docker rootful ; restrictions/allocations, timeout, copie/export, callback rejeté et cleanup ciblé. Pas saturation, audit E2E ou reprise daemon. Le rapport/revue conservent aussi leurs versions et essais antérieurs. |
| [Smoke final réel](runtime-smoke-final.log), [revue finale](review-runtime-smoke-final.md) | 1 test réel PASS via broker/gateway, original préservé, export neuf et refus sans marqueur/conteneur/volume de sa session. Demandes successives, pas trois workers simultanés. |
| [Navigateur final réel](browser-live-final.log), [rapport](browser-live-report.md), [revue finale](review-browser-live.md) | 9 contrôles PASS sur les vrais services, dont auth/bundle/UI/SSE/reconnexion/export/logout/fermeture ; deux événements QA déclarés, aucun effet runtime/fournisseur. |
| [Tests front finaux](browser-unit-final.log), [contrôle d'artefact](browser-evidence-check.log) | 17 tests PASS ; export QA validé, 1 689 octets, deux événements, aucun credential trouvé par les contrôles du harnais. Le JSONL métier reste dans son répertoire temporaire, sans copie ici. |
| [FAIL navigateur SSE](browser-live-fail-sse.log), [FAIL comparaison export](browser-live-fail-export.log) | Échecs historiques du harnais, corrigés et relus avant le PASS final ; aucune réécriture des résultats précédents. |
| [Revue ingress finale](review-ingress-final.md) | Deux P1 et deux P2 fermés après revalidation indépendante ; logiciel/auth/délais/garde/healthcheck. Ce relais d'entrée ne constitue pas le relais inter-workers. |
| [Inspection ingress](ingress-runtime-inspection.log), [routes ingress](ingress-runtime-routes.log) | Bindings loopback, mounts vides, profil et routes du composant de publication. La route par défaut d'ingress est attendue. |
| [Routes gateway](gateway-network-routes.log), [routes dashboard](dashboard-network-routes.log) | Copies de `/proc/net/route` des deux services internes : aucune route par défaut IPv4 observée. Ni IPv6 ni tous les canaux de sortie ne sont qualifiés par ces captures. |
| [Rapport terminal](runtime-terminal-report.md), [revue finale](review-runtime-terminal-final.md) | Résultat terminal inventé corrigé ; Running/ExitCode, états/dates, sentinelle négative et Podman stopped. Les mentions PENDING des gels auteur sont supersédées par la revue finale ciblée. |
| [RED initial terminal](runtime-terminal-initial-red.log), [RED Podman](runtime-terminal-podman-red.log), [GREEN Podman](runtime-terminal-podman-green.log) | Reproductions historiques puis 1 test agrégateur de 36 sous-cas ; exécutables fixtures Rust, aucun moteur Podman réel. |
| [Ciblés terminal](runtime-terminal-targeted-final.log), [Clippy ciblé](runtime-terminal-clippy-final.log) | 20 tests PASS, zéro échec/ignoré et Clippy ciblé PASS ; inclus dans le périmètre logiciel. Persistance terminale avant cleanup toujours ouverte. |
| [Audit avant](audit-before-control-restart.json), [audit après](audit-after-control-restart.json), [commande restart](control-restart.log) | Redémarrage du seul gateway QA sans job actif : JSON byte-identiques, valid=true, 102 événements, même head. Pas crash recovery ou reprise active. |
| [État conteneurs final](control-post-restart-health.log), [gateway healthz](control-post-restart-gateway-health.json), [dashboard healthz](control-post-restart-dashboard-health.json) | Trois services healthy sur l'image nommée, bons endpoints /healthz en succès. Premier contrôle transitoire en échec puis retour sain sans second restart : constat de session de l'orchestrateur. Sondage ultérieur /health=404 : erreur de chemin diagnostique. |
| [Dashboard arrêté](dashboard-health-failure.log), [dashboard rétabli](dashboard-health-restored.log) | Gateway encore 200, healthcheck des deux destinations exit 1 pendant panne dashboard ; après start, endpoints et sonde exit 0. Codes constatés par l'orchestrateur, non imprimés dans les logs courts. Pile rétablie sans suppression de données. |

## Provenance des copies

Chaque copie provient de `/tmp/` ; noms source → noms dans ce dossier :

```text
ai-guard-local-final-clippy.log -> rust-clippy-final.log
ai-guard-local-final-test.log -> rust-tests-final.log
ai-guard-local-final-release.log -> rust-release-final.log
ai-guard-smoke-final-clippy.log -> smoke-delta-clippy-final.log
ai-guard-smoke-final-fmt.log -> smoke-delta-fmt-final.log
ai-guard-final-runtime-confinement.log -> runtime-confinement-final.log
ai-guard-real-smoke-final.log -> runtime-smoke-final.log
ai-guard-review-smoke-final.md -> review-runtime-smoke-final.md
ai-guard-runtime-resume-report.md -> runtime-confinement-report.md
ai-guard-review-runtime-confinement.md -> review-runtime-confinement.md
ai-guard-browser-live-real-20261029-8bd72c0e-after-export.log -> browser-live-final.log
ai-guard-browser-live-real-20261029-8bd72c0e.log -> browser-live-fail-sse.log
ai-guard-browser-live-real-20261029-8bd72c0e-after-transport.log -> browser-live-fail-export.log
ai-guard-browser-live-unit-after-export.log -> browser-unit-final.log
ai-guard-browser-live-real-evidence-check.log -> browser-evidence-check.log
ai-guard-browser-live-report.md -> browser-live-report.md
ai-guard-review-browser-live.md -> review-browser-live.md
ai-guard-review-ingress.md -> review-ingress-final.md
ai-guard-runtime-terminal-report.md -> runtime-terminal-report.md
ai-guard-review-runtime-terminal.md -> review-runtime-terminal-final.md
ai-guard-runtime-terminal-red.log -> runtime-terminal-initial-red.log
ai-guard-runtime-terminal-podman-red.log -> runtime-terminal-podman-red.log
ai-guard-runtime-terminal-podman-green.log -> runtime-terminal-podman-green.log
ai-guard-runtime-terminal-targeted-final.log -> runtime-terminal-targeted-final.log
ai-guard-runtime-terminal-clippy-final.log -> runtime-terminal-clippy-final.log
ai-guard-audit-before-control-restart.json -> audit-before-control-restart.json
ai-guard-audit-after-control-restart.json -> audit-after-control-restart.json
ai-guard-control-restart.log -> control-restart.log
ai-guard-control-post-restart-health.log -> control-post-restart-health.log
ai-guard-control-post-restart-gateway-health.json -> control-post-restart-gateway-health.json
ai-guard-control-post-restart-dashboard-health.json -> control-post-restart-dashboard-health.json
ai-guard-ingress-inspection.log -> ingress-runtime-inspection.log
ai-guard-ingress-routes.log -> ingress-runtime-routes.log
ai-guard-dashboard-health-failure.log -> dashboard-health-failure.log
ai-guard-dashboard-health-restored.log -> dashboard-health-restored.log
ai-guard-gateway-routes.log -> gateway-network-routes.log
ai-guard-dashboard-routes.log -> dashboard-network-routes.log
```

Les 37 copies sont vérifiées avec `cmp` contre leurs sources au gel. [SHA256SUMS.txt](SHA256SUMS.txt) permet de contrôler l'intégrité des copies et de cet index. [SOURCE_SHA256SUMS.txt](SOURCE_SHA256SUMS.txt) conserve les empreintes de sources/tests/configurations du candidat nommé et relu, avant le lot de conservation terminale suivant ; il ne prouve pas à lui seul leur incorporation dans une image reconstruite.

Les rapports copiés conservent leurs liens externes, chemins temporaires et chronologie d'origine. Certains chemins référencent des preuves intermédiaires non copiées ici ; leur absence dans ce dossier n'est pas convertie en PASS. Aucun credential réel, capture, HAR, clé ou export métier n'est ajouté. Une recherche de motifs de credentials connus ne constitue pas un détecteur universel de secrets ; les contrôles du harnais et la revue restent décrits avec leur portée.
