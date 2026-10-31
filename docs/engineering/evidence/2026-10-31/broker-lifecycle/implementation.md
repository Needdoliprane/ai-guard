# Lifecycle local du broker — GEL1 applicatif

Base4890df13f56f2a61d9ee773f9d1dbd7cca929e72, worktree broker-service. Contrat canonique : dernière section BROKER_JOURNAL.md, C1/C2 et getters empruntés confirmés par root. Aucune source hors périmètre autorisé modifiée ; docs/tests préexistants conservés. Aucun commit.

## Implémentation

- lifecycle.rs : BrokerCheckpointV3 enveloppe V2 inchangé et commandes locales fermées. Constructeurs/getters, Clone, sérialisation/désérialisation opaques strictes, EOF/bornes, commandes append-only/UUID uniques/hash recomputé contre opération+intent_hash réels. Maximum64 commandes/64KiB, enveloppe32MiB. ApprovalSubmitted/CancelRequested/FailureObserved ; codes et phases fermés. Le wire CancelRequested utilise une variante struct vide pour refuser les champs inconnus également derrière le buffering enum.
- Approbation : Review ACK exact dans la révision précédente ; aucune Admission/cancel antérieure ou co-née ; au plus une approbation, runner non approuvé à sa naissance. Le false→true exige la commande précédente et un nouveau Decision Approved pending. Les approbations V1/V2 déjà confirmées sont conservées sans histoire humaine fabriquée.
- Cancel : toutes interdictions s'appliquent dès le premier candidat qui contient CancelRequested. Requêtes/ACK antérieurs restent soumis aux contrôles V2 ; nouveaux Decision/Admission/Prepare/CreateIssued/StartClaim/WorkerClaim et aides hors Export sont bloqués. Owned worker tardif nécessite CreateIssued déjà présent précédemment ; owned helper nécessite sa préparation antérieure via transition V2. Export postmortem reste soumis au véritable ExportRequest ACK/permission/terminal V2. Aucun create/claim réellement émis par ces DTO.
- journal.rs : reject avec même ticket owner/lease/op, mutex/flock, liste exacte pré-rejet, O_EXCL et fsync fichier/job/jobs ; replay strict reservation+rejected et plancher hash partagé. Partiels conservés, dual marker Corrupt. Stages cfg(test) RejectedWritten/RejectedDurable et BeforeAck existant. Aucun nouveau mécanisme de persistance.
- status_locked commun évite tout lock récursif. Planchers Accepted/Rejected vérifiés avant absence/Interrupted/Rejected ; ancienne régression accepted.json disparu ne retourne plus Ok(Interrupted). list inventorie tous UUID/types/caps, vérifie tous planchers avant pagination et valide tous jobs déjà observés même hors page ; seuls les nouveaux corps retenus sont chargés. Curseur exclusif/limit1..100 sans snapshot inter-pages.
- Métadonnées de page typées sans FD/secret/path ; pending_rpc_count=None en V1 et nombre de réponses absentes en V2/V3. Worker/runtime/dernier observed_at proviennent du Worker courant stocké, jamais d'une inspection fraîche. load reste accepted-only, rejet complet→InvalidInput.
- Chaîne V3 : même publication rev/current/CAS/caps, domaine checkpoint:v3 distinct. Rev0/V1/V2/domaines historiques inchangés. Refus V1/V2 après V3 avant tout replay ancien ; replay V3 exact retourne reçu historique. command_receipt est dérivé de la première révision confirmée contenant l'entrée ; aucune écriture ni ACK central fabriqué. Getter V2/runner conserve la vue typée intérieure. Aucun cache FS ajouté.
- checkpoint.rs : seule modification, visibilité crate du prédicat pur has_response existant. checkpoint/validation.rs est inchangé ; ses SHA figurent au gel pour preuve du périmètre. broker/mod.rs ajoute seulement l'export lifecycle.

## Sources GEL1

src/broker/lifecycle.rs d0298b9c322b26a9de0b9b10d91fde55987376625930f445b6aa4810ae663aef
src/broker/journal.rs 6cf5a6a5481315bdaa95a62a3e5cae2f84848010e25de3bdd8e2fc2c7b579bd9
src/broker/journal/types.rs a6f70ad3e65a004deaac75d29ae70cfe109229a658a91a7a82f219095ec6fd78
src/broker/checkpoint.rs 2319c6a3867496f0fbf90b7fe97b8992b9c300d23bd38917e46f075d2bf8f104
src/broker/checkpoint/validation.rs 5733a07b6fc34ccdac6512fc366202291a61cb48f428ece3832ec9c486d3cf15
src/broker/mod.rs 51012997653d6f4c53d25de956a8a8d718ca14bc93e3000a58898a182a073504

## Preuves et limites

PASS constatés par cet agent : rustfmt --edition2024 --config skip_children=true des cinq fichiers modifiés ; git diff --check sans sortie. Les six sources exactes sont copiées ci-dessous avec manifeste SHA dans un répertoire privé.
RED préalables confirmés par root : floor réel réserve Ok(Interrupted) après perte de accepted.json, /tmp/ai-guard-journal-lifecycle-floor-red/cargo-red-diagnostic.log ; API absente RC101, /tmp/ai-guard-broker-lifecycle-red/cargo-red-no-run.log. Aucun exploit runtime déduit.
NOT_RUN par cet agent : compilation, tests, Clippy, moteurs, fournisseurs, IPC/service. Luna possède Cargo et doit qualifier ce gel ; aucune réussite logicielle encore revendiquée.

Ces faits locaux n'authentifient pas le caller, ne valent ni Allow, ni terminal runtime, ni libération de quota. Preparing sans CID reste une impasse de réconciliation sous les règles Store existantes. Le service doit empêcher les effets par son cancel mémoire monotone/dernier garde et maintenir l'enforcer après échec writer/HTTP/caps ; aucun de ces mécanismes n'est livré par ce lot de données. Aucun cutover CLI, anti-restore entre processus ou fermeture D5 revendiqués.

Copie privée exacte des sources GEL1 :
/tmp/ai-guard-broker-lifecycle-gel1.6aXQOu

## GEL2 — trois corrections de revue confirmées

Cette section remplace les affirmations GEL1 relatives à la validité de tout ancien grant Export et à l'exhaustivité de l'ensemble de jobs observés. La compilation applicative GEL1 et le canari floor1/1 GREEN sont rapportés par root, pas exécutés par cet agent. Trois régressions ont ensuite été confirmées avant correction et tous Cargo étaient terminaux : ancien grant Export pré-cancel accepté (/tmp/ai-guard-broker-lifecycle-export-red/gel1-export-cancel-release.log), réservation observée corrompue hors page masquée (/tmp/ai-guard-broker-lifecycle-gel1-tests/list-observed-outside-page-diagnostic.log), diagnostics publics des enums rejet non opaques (rejection-enum-red.log).

Delta GEL2 :

- Le loader dérive et retient en interne last_cancel_operations, longueur de la liste d'opérations à CHAQUE nouvelle annulation confirmée ; aucune donnée wire ajoutée. Les avances et la lecture de toute la chaîne valident la même frontière. Une nouvelle ExportRequest co-née avec Cancel est refusée. Toute nouvelle préparation/Arm/HelperClaim Export doit retrouver le receipt de son grant dans un ExportRequest dont l'index de naissance suit la dernière frontière ; ACK tardif d'ancien request et ancien Arm ne contournent pas ce contrôle. Les réponses tardives des requêtes déjà pending restent des faits, jamais de nouveaux droits d'émission.
- Created factuel tardif et premier owned worker post-cancel exigent CreateIssued déjà durable avant cette frontière. Les règles Store, versions V1/V2, hashes historiques et schéma V3 restent inchangés.
- Le writer conserve un ensemble observed_jobs borné au cap jobs, partagé entre clones. Les réservations rendues New et statuts ReservedInProcess/Interrupted réellement observés y entrent, ainsi que les résultats accepted/rejected. Aucun status(None) n'y entre. Avant pagination, tous IDs/planchers sont comparés à l'inventaire et leurs statuts relus ; l'ensemble est dédupliqué pour éviter des chargements supplémentaires des jobs accepted/rejected. Disparition d'un job connu donne Conflict ; capacité mémoire atteinte donne LimitExceeded sans oubli silencieux. Le set n'est pas persisté et ne revendique aucun anti-restore après restart.
- RejectionPhaseV1 et RejectionCodeV1 gardent leur wire snake_case et utilisent désormais des Deserialize publiques à erreur statique opaque.

Sources GEL2 :

src/broker/lifecycle.rs c6b75bd0c7eba175cf1c1bfaa5d5095cae939fc775f44eca97bca22abbb09e7f
src/broker/journal.rs ead8b0726fd18c96df8f936027d7f33ac121d5a2f12289144a6a17c18f07f6b3
src/broker/journal/types.rs 172f76f83dc262ea90bc4ad21ff4179da6d1b4a08e4cae7582abc3e25236bac3
src/broker/checkpoint.rs 2319c6a3867496f0fbf90b7fe97b8992b9c300d23bd38917e46f075d2bf8f104
src/broker/checkpoint/validation.rs 5733a07b6fc34ccdac6512fc366202291a61cb48f428ece3832ec9c486d3cf15
src/broker/mod.rs 51012997653d6f4c53d25de956a8a8d718ca14bc93e3000a58898a182a073504

PASS local : rustfmt ciblé skip_children des trois fichiers modifiés et git diff --check sans sortie. NOT_RUN : Cargo/compilation/tests/Clippy chez cet agent. Luna doit attendre le gel runner de root en plus de ce GEL2. Aucun ajout scope AcceptedJob, aucune édition runner/runtime/API/audit ; aucune modification docs/tests par cet agent. Toutes limites de service/enforcer/D5 et quota décrites plus haut restent ouvertes.

Copie privée exacte des six sources GEL2 :
/tmp/ai-guard-broker-lifecycle-gel2.IGgtI8
