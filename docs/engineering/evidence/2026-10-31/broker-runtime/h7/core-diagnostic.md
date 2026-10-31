# H7 — Arm worker engagée, refus préclaim non diagnostiqué

Produit D3 `99238e3f38ac851a0a98813ec24ef9c5a2cea950`, seuls budgets du harnais120/240s changés. Lectures ciblées après signal terminal Root : journal JSON whitelist, `sqlite3 -readonly` (SELECT types/UUID/états/seq/champs diagnostic), stat et scalaires Docker déjà capturés par Luna. Aucun Store ouvert, Cargo, moteur, fournisseur, source ou QA modifié par ce reviewer. Rectification documentaire : l'en-tête de mon rapport H6 ajoutait un zéro à ce commit ; les faits H6 restent inchangés.

## Faits

QA `/private/tmp/br-yEdg2N`, job `dca89d66-e46a-4eec-90b3-8cceace99194`, sous `b/journals/g09/broker-journal-v1/d4e0a83393e2dee64d54c0902016818f6085306a327da66035870b43e004c019/jobs/<job>/`. `current.json` pointe rev34/hash `717b472fe70b348e531afff056d0b656dcc96fa0165643976c097f9fefb8a799` ; lecture de projection, sans nouvelle ouverture applicative de la chaîne.

- **14/14 opérations supervision ACKées, zéro pending.** Aucun événement `broker.batch.failure` local ou SQLite ; aucune V4. SQLite16 événements, zéro allocation.
- Dernier ACK : **WorkerArm rev33**, `ac4dca97-3fcf-41b6-9d53-fd3ce82bbfc5`, SQLite seq16 reçu19:06:14.785Z.
- **Aucun WorkerClaim**, même pending. Cette frontière diffère de H4/H5, où le claim avait été engagé.
- Première et seule FailureObserved : rev34, `b49da672-323c-4b5f-9a22-0b0473bbbe8c`, phase `worker`, code `unavailable`.
- Worker `fecfaf08-5402-4f7d-bb26-afd4b008b0b5` : Created, revision4, quota retenu. Attempt `34d6d289-f365-43d4-a066-aa48c1f5acb6`. CID `229b3c948ed9048001362df0acc192414af3a5cb603c038dd553fddfd314316a` ; create op `7b62fcd3-8bf5-43b4-a4dd-912d3c5f1d8a`, aucun start op.
- Import et vérification/cleanups sont ACKés. Les scalaires Luna (`/private/tmp/ai-guard-runtime-h7.Jk2RRo/h7-container-scalars.txt`) constatent le même worker Created, `running=false`, aucun StartedAt/FinishedAt, restart0. L'ExitCode0 d'un Created n'est pas un terminal.

Arm worker : timeout5s, native sample `343733635626041`, deadline `343738635626041` ns, méthode Darwin, boot `df920bcd-1409-46e1-844a-47ba7ceb0e6c`, observed_utc null. Aucun sample d'erreur ni stage/reason n'est disponible.

Mtimes epoch : pending32 `1793473574.161340113`, ACK33 `1793473579.058683342`, Failure34 `1793473585.507432199`. L'écart entre écritures pending/ACK est4,897343229s, sans le travail antérieur ni le postload. Cela rend une expiration compatible, **pas démontrée** : mtime n'est pas l'horloge continue du check. Stage Submit `1793473474.769237022`, résultat FAIL publié `1793473595.404435892`, qui n'est pas l'instant exact de l'erreur. Log H7 ligne8 : borne préRunning atteinte, durée122,78s. L'Arm worker et la Failure sont antérieures au teardown du harnais ; H7 ne se réduit donc pas au seul retard de préparation constaté dans H6.

## Cause établie et limite

Le premier défaut est borné **après ACK Arm, avant publication du claim**, pas à Prepare/BeforeSpawn. La source appelle alors `RegistrationV1::new` → `enforcer.register` → `self.load()` → contrôles generation/ready → sample claim (`executor/helpers.rs:89–125`). `check_ready_for_claim` renvoie encore des codes génériques (`enforcer/client.rs:727–750`). Une erreur à ces points n'est pas classée par D3 Prepare/BeforeSpawn ; la classification peut légitimement être vide. L'absence de diagnostic ne désigne donc pas un motif particulier.

Correction comportementale non justifiée à ce stade. Le lot diagnostique utile est de coder cette seule fenêtre préclaim (construction/Register, lecture courante, ready-for-claim/sample), en gardant les codes publics, l'ordre et les mêmes lectures/échantillons ; produire une raison exacte sur erreur seulement. Pas de log libre, deadline étendue, nouveau claim ou nouvelle permission. Le contrôle ready doit rester immédiatement avant la requête du claim, y compris après l'écriture pending (`executor/journal.rs:195–199`).

## Une optimisation conditionnelle à garanties équivalentes

Si une expiration imputable à cette fenêtre est ensuite prouvée, le premier candidat est **la lecture Full redondante de l'adaptateur Writer**, pas le retrait de fsync. `writer.rs:867–885` charge tout le job uniquement pour adapter V3 à V4, puis `Journal::advance_checkpoint` refait son préload strict1025 et son postload strict1075. Chaque load valide et sync l'historique0..head (`journal.rs:1369–1393`), avec revalidation des fichiers avant/après `persistent_sync` (`control_fs/persistent.rs:627–633`).

Couture proposée : déplacer l'adaptation V3→V4 dans une méthode Journal interne appelée par Writer, **sous le même Mutex et à partir de l'unique préload strict déjà requis**, puis réutiliser le publisher et le postload existants. Journal.advance_lifecycle public garde son refus direct après V4. Préserver outbox exacte, CAS attendu, contrôles secrets/binding/caps, confirmation Full pré **et** postpublication, floors, Unknown et barrières physiques. Pas de cache ApprovedJob ni de preuve de sync inférée du seul hash. Fichiers candidats limités à `journal.rs` et `writer.rs`.

Régressions : CAS ancien refuse sans perdre événements/ACK V4 ; binding perdu au hook postpublication reste IoUnknown ; mêmes reçus/heads sur replay ; même erreur de barrière réelle refuse ACK. Une trace cfg(test) placée après opérations réelles peut montrer qu'un seul load d'adaptation disparaît, sans représenter une qualification de disque. Ce candidat réduit une répétition de garanties déjà présentes ; H7 ne mesure pas encore son gain ni sa causalité et **n'autorise pas son implémentation**.

Hypothèse additionnelle non mesurée : les gardiens retenus continuent les contrôles ciblés à250ms après échéance, même pour des helpers déjà nettoyés (`enforcer/server.rs:452–463`). Cela peut contribuer à la charge cumulée ; aucune suppression de cible ou extinction n'en est autorisée. D5 et causes H4/H5 restent ouverts. SIGKILL après Running et autonomie à échéance : **NOT_RUN dans H7**.
