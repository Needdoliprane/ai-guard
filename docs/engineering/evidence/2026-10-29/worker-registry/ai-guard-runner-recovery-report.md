# Reprise du broker workers v2 — 29 octobre 2026

Portée : reprise du lot précédent, parcours CLI `worker-status`, `worker-resume`, `worker-stop`, import/export via runtime managed et tests de fautes. Les tests nouveaux utilisent la CLI de production, un vrai serveur HTTP loopback et SQLite, avec une fixture moteur externe écrite en Rust. **Ils ne qualifient aucun moteur Docker/Podman, plateforme, confinement ou déploiement cloud.**

## Propriété et état du checkout

Lecture des instructions AGENTS.md et RTK, du contrat d'architecture, des règles de code, des gates, de la stratégie de tests et des contrats tenancy/usage/recette. Le dépôt était intégralement non suivi, sans base de diff utilisable ; le travail existant a été conservé. Aucune suppression de dépôt, volume, preuve, donnée ou secret. Aucun endpoint cloud/NAS/PKI utilisé.

Périmètre attribué : `src/runner.rs`, `src/runner/managed.rs`, `src/operations.rs`, `tests/runtime_authorization.rs`, `tests/runner_terminal_audit.rs`, `tests/runtime_terminal_persistence.rs`, `tests/runtime_fixtures/runner_terminal.rs` et nouveau `tests/runner_recovery.rs`. `tests/runtime_smoke.rs` a été cédé au root pour ses vérifications positives réelles après mon gel. `src/runtime.rs` et `src/runtime/managed.rs` n'ont pas été modifiés par cet agent. La machine à états Store appartient à registry_recovery.

Le fichier `.agents/skills/ai-guard-verify/SKILL.md` est absent ; aucun chargement automatique de ce skill projet n'est revendiqué. Méthode appliquée : diagnostic, reproductions RED, corrections ciblées et vérifications avant bilan. La règle historique de skill proposant d'effacer du code existant ne remplace pas l'obligation AGENTS de préserver les travaux.

## Corrections et comportement constaté

- Après perte de l'accusé du `create` moteur, le checkpoint conserve le create intent durable. `worker-resume` peut retrouver uniquement l'instance par inventaire ciblé, ID complet et validation stricte de sa configuration/labels/image/volume/commande. Un worker InDoubt sans Created/Start peut alors acquitter Created et obtenir son **premier** claim start. Aucun nouveau create n'est émis.
- Si l'accusé HTTP Created est perdu, `mark_unknown` conserve l'opération Created en attente avec son identité originale. Émettre d'abord Unknown ferait avancer la révision et empêcherait la reprise d'un Created encore non committé, ou masquerait un Created déjà committé. La reprise rejoue exactement ce pending puis rafraîchit l'état avant toute émission.
- Si le claim de start a été committé mais son accusé perdu, la capacité reste réservée et le worker reste InDoubt. L'instance Created n'est jamais redémarrée, même après deux reprises. Aucune allocation n'est inventée.
- Une préparation InDoubt peut reprendre avant toute émission de helper lorsque le checkpoint helper est absent : celui-ci est durable avant create helper. Le chemin exige quota détenu, aucun create/CID/CreatedInspection/start, aucune incohérence d'allocation, guard OS courant, archive privée inchangée et manifeste admis. Le volume éventuellement déjà créé passe toujours la vérification de propriété runtime. Une demande d'arrêt pendant préparation bloque cette reprise.
- Un helper import terminé avant l'acquittement de fin se reprend par son ID/opération/configuration, succès terminal et comparaison réelle de l'archive du volume au manifeste admis. Aucun nouvel import ne réécrit le volume. La fixture conserve maintenant les octets tar reçus et les restitue pour cette comparaison ; ils ne sont pas des données produit fictives.
- Après SIGKILL du processus broker dans le test logiciel, la reprise observe l'instance running et signale que l'attach output est inconnu. L'arrêt retrouve le même CID et finalise un unique ledger terminal non facturable, puis le replay ne redémarre rien.
- Un broker actif garde le verrou OS : une deuxième CLI resume est refusée. La CLI stop peut cependant acquitter une intention d'arrêt durable ; le broker propriétaire la traite et termine le même worker. Ce test utilise réellement deux processus CLI et flock.
- Autre origine, credential authentifié d'un autre tenant, binding d'autorité divergent et digest de manifeste modifié sont refusés avant effet moteur. Archive privée substituée après import interrompu : refus avant nouvelle émission. Le projet source reste identique.
- Le test unitaire de verrou exigeait désormais une racine privée 0700 mais créait un TempDir de mode ambiant ; reproduction puis chmod 0700 du fixture de test. Aucun assouplissement de contrôle produit.
- La fixture moteur concurrente écrit chaque enregistrement append en un `write_all` et publie son fichier d'état par rename ; cela évite l'entrelacement de lignes ou l'observation d'un état transitoirement tronqué entre ses processus start/inspect.

## Preuves exécutées

PASS — RED initial reproduit pour les deux pertes d'accusé create : `/tmp/ai-guard-runner-recovery-red.log`. La perte d'accusé claim était déjà correctement conservatrice ; l'assertion de test a été corrigée pour exiger le code CLI non nul attendu.

PASS — RED de préparation InDoubt avant tout helper : `/tmp/ai-guard-runner-recovery-prepare-red.log` (reprise bloquée avant correction).

PASS — `CARGO_INCREMENTAL=0 cargo test --locked --test runner_recovery --test runner_terminal_audit --test runtime_authorization` : recovery 6, terminal 1 (matrice de 9 fautes), authorization 6, zéro échec/zéro ignoré. Source finale de ce périmètre : `/tmp/ai-guard-runner-recovery-final-targeted.log`.

PASS — `cargo clippy --locked --all-targets -- -D warnings` sur l'intégration disponible : `/tmp/ai-guard-runner-recovery-final-clippy.log`, exit 0. Des modifications indépendantes de registre peuvent suivre ; le root relance ses gates après intégration.

PASS — rustfmt ciblé sur les fichiers possédés. Le root garde `cargo fmt --all -- --check` de l'intégration.

FAIL constaté puis corrigé — première suite globale : un nouveau test de frontière comptait 0 starts import au lieu de 1, alors que le ciblé passait. Le journal concurrent de la fixture pouvait entrelacer ses records, et son état était écrit par troncature. Les publications de la fixture ont été rendues atomiques et l'assertion exige maintenant explicitement que la faute import soit atteinte. Log historique conservé : `/tmp/ai-guard-runner-recovery-suite.log`.

PASS — dernier `CARGO_INCREMENTAL=0 cargo test --locked`, exit 0 : 201 tests exécutés, zéro échec et 3 tests moteur ignorés (NOT_RUN, jamais comptés PASS). Log `/tmp/ai-guard-runner-recovery-final-suite.log`. Rust et Cargo réellement utilisés : 1.99.1.

## Limites et écarts

- NOT_RUN par cet agent : Docker/Podman réels, plateformes Linux/macOS/ARM64, Kubernetes/cloud, charge de 100 workers, PKI/NAS, release. Le root mène les preuves Docker séparément. La preuve réelle terminal 503 déjà fournie par le root reste celle de `/tmp/ai-guard-workers-v2-real-terminal-initial.log` ; elle ne devient pas une preuve de reprise complète par les nouveaux tests fixture.
- Écart P1 ouvert : limite de durée autonome du worker après SIGKILL du broker. Le test de crash ici arrête explicitement l'instance retrouvée ; il ne prouve pas son arrêt automatique à l'échéance sans broker.
- Une identité moteur inconnue, un inventaire en échec, un helper ambigu ou un claim start consommé sans démarrage observable conservent quota et preuves. Aucune disparition n'est convertie en terminal, durée0 ou droit de relancer.
- Sorties attach perdues lors d'un crash ne sont pas reconstruites depuis un faux résultat ; elles restent inconnues. La réconciliation terminale utilise les timestamps runtime, jamais le temps attach comme mesure d'allocation.
- Second regard indépendant sur les corrections de lancement/reprise requis. registry_recovery a été sollicité mais est occupé au P1 d'intégrité replay Store ; le root coordonne cette revue.
- Le lot et ses preuves logicielles ne ferment pas les gates G01–G12 et ne qualifient pas une release production.

## Contrat d'intégration utile au root

`worker_status` renvoie une enveloppe JSON `{session_id, tenant_id, gateway_origin, worker, runtime_observation, registry_observation_scope, outputs, message, result}`. Une reprise pure d'observation ou après terminal/cleanup renvoie la même enveloppe. Une reprise qui effectue le premier start récupéré renvoie le `RunSummary` direct. `worker` est un `WorkerV1`. Une CLI resume/stop retournant un worker `in_doubt` affiche son JSON puis sort en échec avec diagnostic ; ceci est intentionnel.

Sources principales de revue : `src/runner/managed.rs` fonctions `mark_unknown`, `launch_worker`, `resume_worker`, `recover_import`, `stop_worker` ; nouveaux tests `tests/runner_recovery.rs`.
