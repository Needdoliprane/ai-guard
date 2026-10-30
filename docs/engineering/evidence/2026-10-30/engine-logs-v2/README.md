# Sorties moteur sans journal brut — 30 octobre 2026

Lot **implémenté, vérifié et déployé en QA**, avec revue indépendante ciblée. Ce n'est pas une qualification production ni un nouvel audit Daybreak. Les verdicts de l'audit figé restent inchangés.

## Contrat livré

Le broker impose `--log-driver none` aux nouvelles créations et inspecte le résultat avant claim/start. Le preset `fixed_sandbox_v2` garde le confinement batch : `Tty=false`, `OpenStdin=false`, `StdinOnce=false`. Les helpers ont aussi le journal désactivé ; seul l'import ouvre stdin, avec la différence Docker/Podman explicitement contrôlée. La capture volontaire du broker reste distincte et masquée avant persistance.

Le [constat initial](baseline.md) observe **local**, avec rotation native par défaut, sur l'ancien worker Docker testé : cinq octets lisibles, contenu non affiché. Il ne démontre ni rétention illimitée sur cet hôte ni fuite de secret. La correction ferme le canal de conservation brute du moteur pour les nouvelles créations inspectées ; les quotas bytes/inodes restent ouverts.

Les anciens JSON/hashes v1 restent exacts. Un premier start v1 est refusé avant claim ; une ancienne session déjà partie reste consultable, arrêtable et finalisable sans second start. L'API accepte toujours une admission v1 explicite d'un runner de confiance : la garantie exige le nouveau broker et le preset v2 inspecté. Voir [core](core.md), [runtime](runtime.md) et [API](../../../../api.md).

## Résultats exécutés

| Statut | Preuve et portée |
|---|---|
| RED attendu | Ancienne source327 isolée : golden historique v1 PASS, deux scénarios v2 FAIL sur variante inconnue. L'essai initial sur checkout en évolution ne compilait pas et ne sert pas de preuve comportementale. |
| PASS ciblés | Lib log-policy/first-start ; registry35, API6, runtime-security12, retention1, terminal1, recovery16, terminal-audit1. `runtime_managed` :13 PASS au premier passage, puis le scénario corrigé PASS séparément. Pas de nouvelle suite globale prétendue. |
| PASS | Fmt, Clippy all-targets strict, release. Après extension du test réel, Clippy ciblé a exigé une portée de MutexGuard plus courte ; fmt/Clippy ciblé repassent, sans changement de commande/assertion/production. |
| PASS Docker | [ACK terminal503](docker-real.md), scénario final9,51s : même instance arrêtée conservée, audit valide après réouverture, quota tenu/InDoubt et allocation absente faute d'ACK. Capture masquée ; canary synthétique absente du checkpoint/API/erreur. `LogConfig=none`, stdout de `docker logs` zéro octet et échec attendu de lecture. Aucun credential réel ni fournisseur. |
| FAIL conservé | La première canary finissait avant l'acquittement du callback de démarrage et ne validait pas la capture. Une seule relance avec commande de test brièvement retardée a validé le scénario. |
| PASS QA | Audit668 et même tête avant/après remplacement des trois services ;19 workers v1 lisibles, tous terminaux, quota tenu0. Sauvegarde à froid privée, volumes/credentials/claims conservés. |
| PASS QA ciblé | [Nouveau `/usr/bin/id`](qa-deployed.md) v2 : UID1000, exit0, terminal durable, cleanup/quota libéré ; une allocation observée `billable=false`. Ancienne session Codex v1 consultée sans mutation. Audit final690 valide. Lecture usage `limit=1000` d'abord refusée400, puis `limit=100` conforme, sans relancer le worker. |
| PASS revue | [Second regard indépendant](review.md), code gelé et preuves recoupés, aucun défaut bloquant identifié. Le reviewer n'a pas exécuté les moteurs. |
| NOT_RUN | Nouvelle campagne navigateur/front (sources UI inchangées), Podman réel, autres plateformes, Kubernetes, charge100, TUI et suite globale après ce lot. |

Les [logs](logs/) incluent les échecs initiaux. Les rapports core/runtime précèdent certains résultats intégrés ; le tableau et la revue finale portent leur statut intégré. Les tests ignorés ne sont PASS que dans l'exécution moteur nommée.

[Contrôle documentaire Luna](docs-check.md) :178 liens relatifs sans cible manquante, hashes recoupés et scan littéral ciblé de40 fichiers sans correspondance. Les ancres n'ont pas été revérifiées par ce contrôle. La phrase éditoriale signalée dans le rapport a été retirée de `qa-deployed.md` après cette lecture ; aucun résultat de test n'a changé.

## Artefacts et environnement

- Source privée : `/tmp/ai-guard-engine-logs-final.mGhUZD/source.tar`, SHA256 `e58e387c47432e7335e73679e87deb1226e68b27827b406addec9b685b72cd18`.
- Binaire macOS : même dossier, `agentsandbox`, SHA256 `a4c2aab40b2b9184dac72301611286eb439fe9cedffffd231c4700db949b7c7a`. [Hashes](freeze.sha256).
- Trois services : `agentsandbox-control:qa-20261030-engine-logs-v2-final-c53bd8`, ID `sha256:33313bfcc385be5056e722cafd891f943e5cdb2ddf3849035a7767d2f1dcf543`.
- Compose `/tmp/ai-guard-registry-qa.40IImn/compose-engine-logs-v2.yaml`, projet `ai-guard-qa-7dce8f12`. Services sains après mise à jour ; dashboard partagé sans montage, UI OVPT inchangée.
- Rust1.99.1 ; macOS27.2 ARM64 ; Docker Desktop4.97.0, CLI29.2.1, Engine29.9.2 Linux ARM64. Exception rootful explicitement autorisée ; aucune qualification rootless déduite.
- Worker générique : `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984`, ID `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`.
- [Avant](qa-before-upgrade.json)/[après](qa-after-upgrade.json) : audit668, tête `971b8923455befad82af3c0d1bad24c8d3b6a4d033e613ad1c2082c0d17fa021` ; après run, audit690, tête `40b460a20ea8bae0783893894d0a014205f1317b251ea8787a0f1755e6b5620b`.

Le premier gel `engine-logs-freeze.zMlToS` reste conservé. Le gel final ajoute uniquement la portée lexicale du MutexGuard du test ; les entrées de build applicatif sont identiques. Les deux instances ACK503 et leurs preuves restent retenues. Aucune suppression de données ni migration SQL.

La durée autonome après mort du broker, la réconciliation/fencing et restauration des claims, le relais/identité worker, la recette A1/A2/B1, les quotas disque, la capacité100 et le cloud restent ouverts. Le CLI interactif est le chantier suivant ; `codex exec` réellement testé ne prouve pas une TUI.
