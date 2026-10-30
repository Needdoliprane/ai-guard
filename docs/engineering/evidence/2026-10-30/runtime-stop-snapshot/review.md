# Arrêt strict par snapshot — revue indépendante du code

30 octobre 2026. Lecture des trois deltas, assertions et logs Luna uniquement. Aucun code/doc du dépôt édité, test, processus, runtime ou fournisseur lancé par le relecteur ; aucun secret réel lu. Hors périmètre : API/HTTP déjà clos et nouveaux contrats/types de supervision.

## Gels et verdict

- `src/runtime.rs` : `b2a371c95892fa5c6c85e2271b2c8073325d5e7d3f292bb611dee406652f134d`.
- `src/runtime/managed.rs` : `6bae8e0b6c0fd3a7d38b832c8fcd556ad5bde6c587ee067c19c29b6e5a719766`.
- `src/runner/managed.rs` : `f5869878a18089865d7b1d3204b6834ce7409d993092ed136a82fc89df5041b8`.
- Baselines comparées : `/tmp/ai-guard-runtime-stop-baseline.obEPFZ/{runtime.rs,managed.rs}` et `/tmp/ai-guard-runtime-stop-snapshot-root-baseline/runner-managed.rs`.

**Correction ciblée ACCEPTÉE. Aucun nouveau P0/P1/P2 reproductible identifié dans le delta.** Le défaut d'arrêt malgré divergence est fermé sur les chemins possédant une baseline, avec RED/GREEN de fixture. Cela ne ferme ni le fencing D5 ni la supervision autonome.

## Contrôles de code

- `runtime/managed.rs:749–805` : le port public valide le handle et son lien CID/nom/version au snapshot. Le port commun exige CID complet, fingerprint présent et syntaxiquement valide, version1/2/3, image immuable, puis une baseline cohérente avec elle-même (nom, image, ressources connues, restart0). Une version absente conserve la sémantique historique v1 ; aucun upgrade/recalcul d'ancien snapshot.
- L'inspection courante passe par l'autorité retenue et `validate_identity_allocation` **avant** tout effet. Erreur/absence/divergence ne tombent pas vers `kill_target`. L'état doit être non-running reconnu, ou running=true avec statut actif reconnu ; un Running inconnu ou incohérent refuse l'effet.
- `kill_target` revalide l'autorité avant la commande. Son erreur n'est pas promue en succès : seule une nouvelle inspection exacte montrant un processus arrêté permet le retour Ok, ce qui accepte correctement une sortie naturelle concurrente. Ni l'exit code CLI ni le seul CID ne remplacent cette preuve.
- `runtime/managed.rs:950–1027` : le fallback d'erreur garde la baseline de reprise ou le Created validé complètement avant callback. Sans baseline, aucun arrêt handle-only de secours. Erreur callback/capture ne remplace pas la baseline par les faits divergents qui ont déclenché l'erreur. Le résultat reste une erreur, instance retenue, diagnostic d'arrêt connu/inconnu distinct.
- `runtime.rs:484–502, 1024–1030, 1153–1164` : timeout/Ctrl-C gérés et garde d'annulation interactive utilisent la baseline. La garde possède sa copie pour la tâche asynchrone ; aucun credential nouveau ni copie de terminal physique n'est ajouté.
- Six raccordements runner confirmés à 1616, 1619, 2537, 2642, 2658, 2931. Stop utilisateur, suivi de stop durable, erreur de refresh et reprise import utilisent les snapshots présents. Recherche des appels source : aucun caller produit `stop_owned` restant ; seuls sa définition legacy et ses détails internes subsistent. `kill_target` est utilisé par le port strict et ce port legacy, sans nouveau bypass.
- Les transactions terminal/ledger/quota et cleanup ne changent pas. `Created/configured/initialized` non-running peut être retourné comme état arrêté, **pas** comme terminal allouable. Le caller `worker-stop` exige encore `validate_terminal` et claim durable avant finalisation ; sinon il garde l'inconnu/quota. Aucun nouveau start, réarmement ou suppression de preuve introduit.

## Preuves recoupées

Rapports `/tmp/ai-guard-runtime-stop-snapshot-implementation.md` et `/tmp/ai-guard-runtime-stop-snapshot-tests.md`, test final SHA `6eb3380bbda0d3afa97170331b5bbbff474b7de5049b6c4a86bbd7edaecb1379`.

- RED `/tmp/ai-guard-runtime-stop-red.xe94YI/target-red-evidence.log`, SHA `8ea696576fbff6f5aecd0334c7d9b58002f319fb51bb5d7df56c3856373600f7` : fixture maintenue Running, divergence config puis logging ; les deux cas enregistrent un kill injustifié. Les erreurs antérieures de compilation du harnais ne constituent pas ce RED.
- GREEN `/tmp/ai-guard-runtime-stop-green.UEbe3q/target-green.log`, SHA `c0a4639e8ea451fbe7d129975e321b359fb467bd121cb1c59300b1863389151d` : **1 test / 6 cas PASS**, 0 ignored. Assertions inspectées : config/logging divergents restent running sans kill ; exact running produit un seul kill et exited ; inspection refusée, Running inconnu, mémoire Created divergente donnent erreur sans kill. Le harnais exécute tous les cas et agrège leurs échecs.
- `runtime-managed-green.log`, SHA `f6f96c6a49b6e39f18810b10a1d409cc4a3afc8d65b9eea7268ddc743f2c250c` : **14 PASS**. Le cas de lancement paramétré inclut erreur callback et timeout ; les attentes config/logging préexistantes, naturellement terminales, n'ont pas été assouplies.
- `runner-recovery-terminal-green.log`, SHA `629a849b2b9e43761dfe47ae34005065ef70e695b6b7a91934e879fc33dd8504` : **16 recovery + 1 terminal PASS**, 0 ignored. Total **32 fonctions de test**, sans additionner les six paramètres comme six fonctions supplémentaires.

La première rédaction du rapport tests attribuait les six raccordements à `src/runner.rs d8ee…`, fichier inchangé ; la bonne provenance est `src/runner/managed.rs f586…` ci-dessus, directement vérifiée et signalée à root. Aucun résultat fmt-all, Clippy-all ou release ultérieur n'est anticipé par cette revue.

## Limites conservées

Inspection puis kill n'est pas atomique face à l'administrateur moteur. La comparaison stricte peut refuser de tuer une instance divergente encore active : c'est le comportement demandé, qui doit rester visible comme arrêt inconnu/réconciliation, sans libération de quota.

Le legacy public `stop_owned(handle)` demeure moins strict et ne doit pas être utilisé par la supervision future. La garde Drop interactive lance une tâche best-effort dans le runtime présent ; elle ne garantit rien après SIGKILL/destruction du runtime. Les lectures FS de vérification d'exécutable et les appels d'autorité/inspection restent potentiellement lents : ce lot ne crée pas de deadline autonome ni de borne globale d'arrêt. Un Created observé arrêté ne fence pas un ancien start RPC tardif.

Les fixtures qualifient l'ordre des opérations et les cas relus, pas l'application par un moteur réel. Les chemins TTY d'annulation ont été relus mais aucun nouveau test réel TTY/moteur n'est attribué à cette campagne. Aucune qualification release, cloud, fournisseurs, gardien, daemon ou D5 n'en découle.

Statuts : revue/code ciblés **ACCEPTÉS**, RED **conservé**, GREEN logiciel ciblé **32 PASS / 0 ignored** ; exécution par le relecteur et qualification moteur réelle **NOT_RUN**.
