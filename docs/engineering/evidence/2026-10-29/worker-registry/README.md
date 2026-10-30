# Registre durable des workers — preuves en cours

Ce dossier distingue le lot du candidat147 précédent. **Aucune qualification de release, Podman, Kubernetes, recette complète A1/A2/B1 ou charge100 n'est revendiquée.** Les premiers éléments ci-dessous sont intermédiaires ; la migration réelle et les résultats du gel209 figurent dans la section suivante. Une preuve intermédiaire n'est pas transférée au code corrigé ensuite.

- `docker-terminal-intermediate.log` : nouveau parcours v2, moteur Docker29.9.2 LinuxARM64 de Desktop4.97.0 sur macOSARM64, rootful explicitement choisi ; 1PASS/0FAIL/0ignoré en3,68s. Vrai gateway Rust/SQLite local avec middleware refusant le terminal HTTP avant Store, même instance arrêtée retenue, quotaheld et ledgerabsent, réouverture valide. Ce n'est pas une panne disque.
- `docker-smoke-intermediate.log` : vrai gateway v2 privé sur loopback, même moteur/imageworker ; 1PASS/0FAIL/0ignoré en10,82s. Deux exécutions, UID1000, copie/export filtrés, refus sans conteneur, allocation observée unique conservée après deux reprises. Instance/services de contrôle de ce test distincts de l'image QA147 ; pas de nouvelle image conteneurisée qualifiée.
- `dashboard-api.log` : 5 tests HTTP/SQLite, dont refus d'une clé runner mal configurée dans le dashboard, modepartagé viewersA/B isolés, mutations refusées, CAS409 et workerA invisible à RunnerB. Aucun moteur n'est exercé.
- `dashboard-cli-red.log` / `dashboard-cli-green.log` : flag partagé explicite, incompatible avec un viewer fixe, modehistorique conservé ; RED avant implémentation puis9 testsCLI+5API PASS.
- `prepare-recovery-report.md` : 30 tests stockage ciblés et limites de la première correction. Les erreurs de vérification globale rapportées appartiennent à ce stade, pas à un gel final.
- `dashboard-identity-report.md` et sous-dossier `dashboard-identity/` : Node27/build/navigateur sur fixtures, identité réelle du contrat API, isolation des retours retardés et mobile ; aucun moteur qualifié.

Le contrôle de durée après mort du broker, la restauration sûre des claims, l'interconnexion métier et ses identités restent ouverts. Les rapports de revue et le manifest du gel209 sont ajoutés plus bas après traitement des défauts. L'absence d'un résultat vaut NOT_RUN, jamais PASS.

Les données/volumes/configurations privées des essais sont conservés hors dépôt. Les credentials ne font pas partie de ce dossier. Les logs publics ne doivent pas contenir leurs valeurs.

## Gel209 et qualification ciblée exécutée après les intermédiaires

`final-fmt.log`, `final-clippy.log`, `final-tests.log`, `final-release.log` : quatre commandes PASS/exit0,209 tests exécutés/0FAIL/3ignorés. `SOURCE_SHA256SUMS.txt` couvre90 entrées ; recheck complet exit0 au gel. `RELEASE_SHA256SUMS.txt` identifie le binaire hôte. Les trois tests ignorés ont leurs exécutions explicites séparées :

| Preuve | Résultat et portée |
|---|---|
| `final-runtime-isolation-terminal.log` | Confinement1PASS/3,75s puis refus terminal1PASS/2,75s. Docker29.9.2/rootful explicitement choisi, LinuxARM64 dans Desktop sur macOSARM64 ; répertoires/volumes/CID de diagnostic conservés. |
| `final-deployed-smoke.log` |1PASS/10,23s sur nouveau contrôle conteneurisé ; UID, copie/export, refus sans conteneur et sans delta de volumes, allocation unique après deux reprises. |
| `final-browser-fixed.log` / `final-browser-shared.log` |10 contrôles PASS chacun sur services réels ; worker explicite lié au smoke, proxy/identity/rôles, bundle, SSE/reconnexion/export et vues workers/quota/allocations. Les injections du navigateur restent déclarées QA. |
| `final-image-build.log` / `deployed-control-inspect.txt` | Image `qa-20261029-workers-31d2c5af`, ID `sha256:39111cab0564e8aa6a11f07772df6c1824bd61c86e1ed25a6b4864b1e17805d8`, trois services healthy. Le dashboard partagé n'a aucun montage. |
| `migration.json` et trois JSON d'audit | Migration réelle offline sauvegardée1→2, historique137 puis genesis138, audit195 valide après essais ; backup et conteneur maintenance conservés. |

Les rapports `ai-guard-worker-replay-report.md`, `ai-guard-registry-integration-review.md`, `ai-guard-runner-lifecycle-review.md` décrivent défauts, correctifs et revue. Leurs NOT_RUN et erreurs de compilation temporaires ne sont pas transformés rétroactivement en PASS : les logs finaux de l'orchestrateur ajoutent les validations manquantes. Les fichiers `ai-guard-worker-replay-*.log` et `ai-guard-resume-exit-*.log` conservent les RED/GREEN correctifs ; aucune exécution de probe par les reviewers n'est revendiquée.

Après ce gel, l'[audit indépendant](../independent-audit/REPORT.md) a été reçu avec19 candidats sur un autre instantané. [AUDIT_FOLLOWUP.md](../../../AUDIT_FOLLOWUP.md) suit leur rapprochement ; ce dossier ne les réfute pas. La source peut ensuite évoluer pour ces corrections : l'image et le manifest ci-dessus restent le candidat209, et non une qualification des éditions ultérieures.
