# Contrat proposé — profil Docker minimal figé au parent accepté

1 novembre2026. Checkout exclusif `/Users/louis/Documents/ai-guard/.worktrees/docker-launch-profile`, branche `codex/docker-launch-profile`, HEAD `82f00d751ed233e1282505000891bad49164ffd0`, état initial propre. Lecture de AGENTS, architecture/règles, BROKER_JOURNAL, BROKER_SUPERVISION, gates/stratégie et `/tmp/ai-guard-d5-launch-profile-design.md`. Aucun source modifié, Cargo/test/build, réseau/moteur/fournisseur, configuration Docker, `.env` ou credential réel ouvert. Le worktree broker-service est hors périmètre.

Cette note est un contrat à approuver avant code. Tous les noms marqués « proposé » sont nouveaux ; ils ne décrivent pas une API déjà livrée. Le profil ne constitue ni témoin de complétion D5, ni fencing, ni retrait de slot, ni nouvelle autorisation métier.

## 1. Formats actuels réellement présents

| Objet actuel | Forme et engagement effectivement utilisés |
|---|---|
| Registre privé | `service/profile.rs::RegistryWireV1`, version1 ; `ControlProfileV1` n'a pas de champ version et ne choisit aujourd'hui aucun profil de start |
| Submit | `SubmitV1`, version1 ; payload exact réservé avant sources/fournisseur ; replay Accepted retourne avant `prepare_broker_submission` |
| Intention | `intent.json` = `ExecutionIntentV1` version1 ; domaine exact `ai-guard.execution-intent.v1\0` ; plans fournisseurs1/2/3 conservés. Aucun type/fichier `StoredIntent` n'existe |
| Credential | `BindingManifestV1` est **transparent**, enum interne taggée kind none/manual/provider, sans champ version. Ne pas lui ajouter un version ou un profil Docker |
| Manifest V1 | `bindings.json` = `{version:1,operation_id,intent_hash,binding:BindingManifestV1}` ; `Accepted.bindings_hash = digest("ai-guard:broker-bindings:v1", canonical_manifest)` |
| Accepted | `{version:1,scope_hash,operation_id,submit_hash,intent_hash,bindings_hash,initial_checkpoint}` ; le code ne calcule **aucun digest broker-accepted** |
| Scope | `scope_hash = digest("ai-guard:broker-journal-scope:v1", canonical(JournalScopeV1))`, sous `broker-journal-v1/<scope_hash>/jobs/<operation>/` |
| Révisions | Formats/domaines broker-checkpoint:v1/v2/v3/v4 ; current version1 ; aucune nouvelle version de payload requise par ce contrat |
| Invocation | `InvocationBindingV1.policy_version=2` pour approbation neuve, fingerprint sous `ai-guard:runtime-invocation:v1\0`. Politique/environment historiques inchangés |
| Autorité | `RuntimeAuthority::fingerprint` est le hash existant de composants longueur-préfixés engine/endpoint/authority_id/Podman ; il exclut invocation. Le conserver exactement |
| Seed actuel | `runner/managed.rs::AuthoritySeed` version1, fichier `<runtime>-<SHA(endpoint)[..24]>-<invocation_fingerprint[..24]>.json` ; partage potentiel entre jobs/scopes d'un même config_dir |

**Conflit documentaire réel :** BROKER_JOURNAL décrit un domaine `ai-guard:broker-accepted:v1` qui n'est présent dans aucun appel/constante source. Cette proposition ne crée pas un domaine fictif pour le justifier. Root corrigera ce passage documentaire dans le lot. `digest(domain,bytes)` ajoute effectivement NUL entre le domaine et les bytes ; les domaines ci-dessus sans suffixe affiché désignent cet helper.

Mise à jour communiquée par Root après livraison de cette note : les domaines documentaires `broker-reservation:v1` **et** `broker-accepted:v1` ont été corrigés dans broker-service seulement. Le checkout82f00d7 reste inchangé ; leur correction sera reprise à l'intégration/mise à jour V2. Les faits source V1 à préserver sont aucun hash propre de ces records, le vrai submit_hash dans reservation et les liens déjà vérifiés dans Accepted.

## 2. Sélection opérateur explicite et compatibilité

Proposition recommandée : ajouter un registre wire version2 avec les mêmes champs opérateur qu'aujourd'hui, et, dans chaque profil, un champ obligatoire fermé :

```json
"prearmed_start_profile": {"version":1,"mode":"docker_minimal"}
```

Mode alternatif explicite proposé pour les installations qui choisissent l'historique : `{"version":1,"mode":"legacy"}`. Aucun chemin/config arbitraire dans ce choix. Une registry V1 reste strictement lue selon son ancien schéma et résout Legacy en mémoire ; elle ne reçoit pas silencieusement le nouveau champ. Une registry V2 sans choix, avec version/mode inconnu ou docker_minimal+Podman est refusée. Ne jamais convertir ce refus en Legacy. Les exemples de configuration neuve sélectionneront explicitement docker_minimal pour Docker ; aucune configuration existante réécrite.

Nouveaux DTO proposés : `RegistryWireV2`, `ControlProfileV2`, `PrearmedStartProfileV1` et une représentation interne normalisée avec choix Legacy/DockerMinimalV1. Conserver les DTO V1 et leur parseur strict ; ne pas employer un flatten qui avalerait des unknown. Leur nom actuel ne vaut pas version indépendante du profil.

Le choix courant du registre sert uniquement à **une nouvelle acceptation**. Une opération Accepted/review/reprise conserve le binding accepté : changement de ce seul choix ne l'upgrade/downgrade pas. Les contrôles actuels de `jobs::require_profile` sur config_dir/gateway/program/endpoint/image/policy/etc. restent applicables. Un programme réellement modifié après acceptation ne remplace pas l'invocation liée : refus explicite, jamais nouveau fingerprint silencieux.

API publiques conservées : `prepare_broker_submission`, `PreparedSubmission::binding_inputs`, `Journal::accept`, `BindingInputs::{None,Manual,Provider}`, getters credentials, `BeforeSpawnGuard::check_before_spawn`, `PrearmedStartControl::new`, plans1/2/3 et helpers runtime historiques. Les nouveaux chemins stricts utilisent des coutures crate-private additionnelles. Aucun nouveau champ à RunOptions/RunPlan/InitialOptions/ExecutionIntent.

## 3. Nouveau binding durable — schéma exact proposé

Seuls les nouveaux jobs docker_minimal écrivent `bindings.json` version2 :

```json
{
  "version": 2,
  "operation_id": "<parent UUID>",
  "intent_hash": "<H_INTENT_V1>",
  "binding": {"kind":"none"},
  "docker_launch_profile": {
    "version": 1,
    "kind": "docker_minimal",
    "journal_scope_hash": "<H_SCOPE_V1>",
    "parent_operation_id": "<parent UUID>",
    "intent_hash": "<H_INTENT_V1>",
    "directory_ref": "docker-launch-v1",
    "file_name": "config.json",
    "sha256": "<SHA256 des trois octets {} LF>",
    "bytes": 3,
    "runtime_endpoint": "<endpoint Unix explicite figé>",
    "invocation_fingerprint": "<fingerprint politique2 approuvée>"
  }
}
```

Le champ binding peut également être l'ancien objet Manual/Provider **identique**, avec ses bytes de credential inchangés. Le blob de profil est indépendant des credentials ; l'existence de None n'interdit pas le profil Docker. Seule la variante DockerMinimalV1 est introduite ici, avec les noms littéraux ci-dessus, aucune référence client libre.

Types proposés : `DockerLaunchProfileBindingV1` (métadonnées Serde fermées/Debug opaque), `StoredBindings` (dispatch interne V1/V2), et une petite sélection préparée non-Serde contenant l'invocation approuvée/endpoint. Aucun `StoredIntent`.

Couplage obligatoire : Accepted version1 ↔ bindings version1/absence de profil ; Accepted version2 ↔ bindings version2/profil obligatoire. Les appels exacts sont `digest("ai-guard:broker-bindings:v1", canonical_manifest_v1)` et **`digest("ai-guard:broker-bindings:v2", canonical_manifest_v2)`**. La préimage V2 est le domaine nu UTF-8, **UN seul octet0 ajouté par digest**, puis les bytes canoniques ; aucun NUL n'est inclus dans l'argument domaine. Un golden indépendant doit refuser la variante à double NUL. Le record Accepted version2 garde exactement ses champs actuels ; son version2 signale la nouvelle paire. Les autres combinaisons sont Corrupt/Conflict sans downgrade. Le lecteur recoupe version, operation, intent_hash, hash exact du manifest canonique et scope ; unknown refusé. Les anciens bytes/domains V1 restent exacts. Un ancien binaire refuse le nouveau format ; aucun rollback avec jobs actifs annoncé.

Le hash de contenu SHA256 est une propriété du fichier ; le nouvel engagement de métadonnées proposé est `digest("ai-guard:docker-launch-profile-binding:v1", canonical(binding_profile))`. Il inclut Scope, parent, intent, refs, version/mode, bytes/SHA, endpoint et invocation. Le SHA constant des seuls octets `{}\n` ne constitue jamais l'identité du profil ou du seed.

`journal_scope_hash` vient de l'Inner Journal attendu après contrôle de la paire Accepted/Scope ; il est comparé à ce contexte, pas accepté comme assertion d'un Submit. C'est une donnée de contexte engagée, **pas une preuve autonome d'identité**. Deux scopes partageant config_dir, UUID/intent/ref doivent produire des engagements et seeds distincts.

## 4. Publication, FD et absence de réécriture

Profil nouveau : `jobs/<parent>/docker-launch-v1/config.json`, exactement `b"{}\n"`, directory0700/fichier0600, eUID, nlink1 et ACL conformes aux primitives actuelles. `directory_ref`/file_name sont des composants générés fixes ; aucun absolu/traversée/lien ou source `.docker`. Le bytes cap est exactement3 ; ref/version/SHA/invocation/endpoint/scope/parent/intent incohérents sont refusés. **Le directory docker-launch-v1 est fermé : config.json seul, aucun autre fichier ni sous-directory.** Tout extra refuse Full/start ; aucun contenu inattendu n'est ouvert, réparé ou supprimé.

`Journal::accept` public reste la voie Legacy. Couture crate-private proposée pour l'acceptation préparée avec launch sélectionné : après validation ticket/op/intent/scope et budget, le Journal construit lui-même le binding, crée exclusivement directory/fichier, synchronise fichier puis directory/job/ancêtres nécessaires, puis publie intent/manifest/rev0/current/Accepted selon l'ordre existant. Le profil doit être durable **avant Accepted et son ACK**, même si la décision ultérieure retourne Review. Le budget compte directory+fichier et3bytes, plus le manifest V2 réel ; aucune limite relâchée. Une erreur/partial garde les preuves et ne donne aucun ACK ni nouveau ticket ; pas de réparation/recréation à un autre nom.

`usage` et `stop_budget_usage` comptent également toutes les entrées/tailles effectivement observables, y compris directory seul, fichier tronqué, extras, orphelins et acceptation partielle. Leurs inventaires incluent les entrées rejetées : une taille ou un sous-arbre inconnu donne couverture/budget inconnu, jamais zéro ni succès de budget fictif. Les fichiers inattendus ne sont pas ouverts ; un sous-directory inattendu n'est pas exploré pour fabriquer un inventaire complet. Voir ci-dessous la limite d'append STOP si cet inventaire ne permet plus de prouver le budget.

Nouveau handle proposé `RetainedDockerLaunchProfile` non-Serde, à champs privés, retenant Directory/PrivateFile et métadonnées validées ; clones éventuels partagent les FD et ne sont pas une permission d'exécution. Full load/replay ouvre uniquement la ref attendue, contrôle bytes/SHA/contextes et retient les FD. Fichier absent, mauvais mode/ACL/link/UID, altération ou parent substitué interdit Full/start. Aucun retour au registre courant ou blob regénéré.

Immutabilité signifie deux choses distinctes : (a) métadonnées acceptées identiques pour toute la chaîne/autorité/guard ; (b) FD/cache/metadata/nom retenus revalidés avant usage par les primitives `persistent_revalidate`. Elles ne promettent ni scellement contre mêmeUID/root ni identité d'inode initiale attestée après un restart. Aucun device/inode source n'est aujourd'hui persisté par ce contrat. Un besoin plus fort serait une exigence additionnelle à trancher, pas une garantie inventée.

## 5. Autorité/seed distincts, STOP indépendant

Ajouter à RuntimeAuthority un champ optionnel `docker_launch_profile: Option<DockerLaunchProfileBindingV1>`, `#[serde(default, skip_serializing_if="Option::is_none")]`, visibilité crate si suffisante. None doit omettre le champ et préserver exactement les anciens JSON, empreintes, Registration digests/goldens et API publiques. `fingerprint()` et `invocation_fingerprint()` restent inchangés.

Un seed strict nouveau version2 utilise un namespace séparé : `docker-launch-v1-<H_SEED>.json`. Définition proposée :

```text
H_SEED = digest("ai-guard:runtime-authority-seed:v2", canonical({
  version:2, kind:"docker", journal_scope_hash, parent_operation_id,
  intent_hash, launch_profile:<binding complet>, invocation_fingerprint,
  runtime_endpoint
}))
```

`AuthoritySeedV2` reprend les champs de V1 plus le binding de launch ; le loader vérifie cette clé ET les champs complets, l'invocation réelle, l'endpoint et la RuntimeAuthority obtenue. Les seeds V1 et leurs noms restent intacts. Un seed du job/scope précédent ne peut pas être réadopté sur la seule égalité endpoint+invocation+SHA({}). Ne pas changer silencieusement le programme d'un strict binding via adopt_invocation : une différence d'invocation stricte nécessite une nouvelle acceptation explicitement préparée.

`ManagedRuntime::reopen/reopen_with_runtime`, `command` et les opérations STOP/inspect vérifient les métadonnées mais **ne chargent pas ce fichier**. Le runtime peut donc être rouvert pour stop si le profil manque. Le handle launch n'est attaché que pour la préparation de start depuis Accepted. Aucun `DOCKER_CONFIG` global changé ; Build/Pull/doctor/historique restent dans leur chemin actuel.

`StopRecoveryReaderV1::read_job`, `StopMetadata`, `StopJournal::append_cancel` et `stop_budget_usage` doivent comprendre bindings/Accepted V2 et le directory supplémentaire. Ils recoupent uniquement les métadonnées engagées ; ils n'ouvrent jamais config.json ni les blobs de credentials. Distinguer strictement :

- **Preuve de contrôle invalide** : manifest engagé altéré, lien Accepted.bindings_hash/version/op/scope/intent invalide, profil de RuntimeAuthority différent du manifest ou chaîne/Arm invalide => projection du job refusée. Aucun mode dégradé ne remplace ces preuves.
- **Défaut du blob** : fichier absent, bytes/digest altérés, mode/ACL/UID/liens du fichier défaillants, parent docker-launch-v1 absent/substitué/non admissible, ou extras dans ce directory => Full/start indisponibles et rapport incomplet/admissions fermées, mais aucun de ces défauts n'est une autorité d'arrêt ni une raison d'effacer une projection dont manifest/chaîne/Arm restent prouvés. Le reader poursuit sans ouvrir/réparer/recréer le blob. Un défaut de l'ancre commune du contrôle (scope/job/records nécessaires eux-mêmes) peut en revanche empêcher de re-prouver cette chaîne : ne pas le confondre avec le seul parent du blob.

`append_cancel` utilise les mêmes métadonnées pures, sans demander le blob, y compris via StopWriterClient/acteur restreint lorsque Full est indisponible. Il conserve néanmoins TOUTES les obligations de budget, quota, audit, durabilité et métadonnées de contrôle avant ACK. Blob absent/altéré avec inventaire metadata complet doit permettre la publication STOP ; fichier/parent dont le défaut rend le budget inconnu nécessite le traitement explicite ci-dessous. Les projections déjà prouvées et l'enforcer vivant restent préservés dans les deux cas ; aucun ACK n'est inventé.

**Limite réelle des primitives actuelles, à résoudre avec Root avant tout contournement :** `stop_budget_usage` (recovery.rs:490) appelle `persistent_entries` strict pour tous les jobs et leurs sous-directories. Il refuse une entrée/parent non admissible ou non énumérable avant publication, même si le reader tolérant a pu prouver la chaîne et ses Arm. Le futur ajout du nom launch ne supprime pas cette limite. Une entrée supplémentaire ordinaire est comptable par metadata ; un fichier absent l'est par son absence ; des bytes modifiés n'exigent aucune lecture du blob pour compter la taille. Mais mode/ACL/lien/subdirectory/parent défaillant peuvent laisser taille/sous-arbre inconnus.

Décision Root : conserver un refus typé de l'ACK append tant que budget ou audit ne sont pas prouvables, tout en poursuivant les STOP déjà autorisés par projections/enforcer ; qualifier cet état comme **écart ouvert**, pas comme acceptation complète du parcours STOP. Le présent lot ne crée aucun contournement, réserve implicite ou nouveau compteur indépendant. Toute conception ultérieure visant à lever cet écart doit être soumise séparément avant code. Aucun saut de scan, taille déclarée3 à la place de l'observation, cap ignoré, quota libérée, audit sauté, lien suivi ou directory unsafe ouvert implicitement n'est autorisé. Disponibilité des projections/enforcer et possibilité d'un append durable sont deux résultats distincts à publier et à tester.

## 6. Parent accepté, helpers et garde finale

`parent_operation_id` est toujours Accepted.operation_id. Ce n'est ni Arm.operation_id, ni claim.operation_id, ni helper_id. Source réelle : `helpers.rs::arm_and_claim` fixe Arm.intent_hash depuis le job et utilise helper_id pour le sujet/handle d'un helper ; `execute_prearmed_retained` et `start_owned_prearmed_retained` des helpers et du worker convergent vers `execute_owned` puis `Runtime::execute_created_prearmed`. Aucun `Registration.logical_operation` n'existe.

Recoupements : Journal vérifie scope+parent+intent ; Execution::open recoupe Accepted avec AuthenticatedBatchContext et Runtime.invocation ; BrokerPreparation/broker_prepare et broker_runtime transportent le même binding/capacité ; le checkpoint managed, ses helpers et chaque Arm gardent ce RuntimeAuthority ; RegistrationV1 valide version/mode/endpoint/invocation du profil et Arm.intent_hash, puis son digest actuel couvre l'autorité **entière**. Le sender contrôle le Scope authentifié avant register/restore : la registration n'est pas une nouvelle authentification humaine. Ne pas exiger parent_operation_id==helper.handle.operation_id ; pour le worker seulement, l'égalité déjà applicable avec intent.operation_id demeure.

Validation de chaîne nouvelle : `confirmed::{context,context_checkpoint,ChainVerifierV1}` reçoit les métadonnées launch attendues du manifest ; conserve les contrôles credentials actuels. **Le binding RuntimeAuthority commun est recoupé au manifest dès qu'il est présent, avant et indépendamment de context.worker/context.owned.** Les helpers portent policy/owned et authority_id/fingerprint, pas chacun une autorité complète. Dès qu'un helper préparé ou Arm existe, la condition actuelle exige déjà context.binding Some : le recoupement du profil de ce binding ne doit pas attendre Created/owned du worker principal. Comparaison complète du profil, pas le seul fingerprint historique qui l'exclut.

L'absence d'autorité au rev0 initial est permise et distincte d'un profil manquant dans une autorité déjà présente. La première RuntimeAuthority d'un job strict doit porter exactement le profil accepté ; celle d'un Legacy doit porter None. Une fois cette autorité présente, changer son profil None→Some, Some→None ou ref/scope/parent/invocation sous le même authority fingerprint est refusé. Le même validateur pur est partagé avec STOP, sans ouverture du blob. Un éventuel wrapper historique du verifier avec argument None signifie **Legacy exact**, jamais « contexte inconnu, vérifier moins » ; les nouveaux lecteurs V2 passent explicitement le manifest réel.

La garde actuelle publique ne prend que CID. Pour préserver cette API, proposition : étendre **les champs privés** de PrearmedStartControl avec l'identité de profil attendue depuis la LocalSlot Registration, garder `new(guard,stop)` Legacy, ajouter un constructeur crate-private lié pour l'enforcer. Avant spawn, un wrapper privé de BeforeSpawnGuard compare en mémoire la capacité préparée/identité attendue puis consomme la garde actuelle CID/génération/clock. Le profil n'est ainsi pas une assertion libre du caller runtime. L'ancien constructeur None face à une autorité stricte ne peut pas émettre un start.

Dans Runtime::execute_created_prearmed, après la dernière préparation awaitée et vérification d'autorité, construire la commande depuis l'invocation approuvée, revalider le handle launch, ajouter `--config <directory retenu>` **avant** `start --attach` et avant la garde finale. Ordre compatible actuel : `--host <endpoint> --config <dir> start --attach ... CID`. Préparer toutes les allocations/stdio/diagnostics et comparisons avant la consommation finale ; **aucune nouvelle IO, await, préparation de commande ou callback entre la garde finale et command.spawn**. Le FD de profil reste vivant jusqu'à la fin du transport. Il n'est pas transmis au worker. Une autorité stricte refuse un chemin Legacy start qui ignorerait ce binding ; elle conserve ses chemins STOP.

Après claim consommé, défaut du profil/guard/spawn garde le claim consommé, l'instance et l'inconnue sous enforcement. Pas de retry start, nouvelle Arm, terminal inventé ou désarmement. L'ouverture ultérieure par pathname du CLI reste dans la frontière DAC/UID actuelle ; aucune atomicité contre mêmeUID/root ni absence de start en vol n'en découle.

## 7. Coutures réelles et périmètre propriétaire

| Source existante | Modification proposée |
|---|---|
| service/profile.rs::Registry::load/ControlProfileV1::validate | parser V1/V2, résolution explicite et refus Podman strict |
| service/server.rs::submit/complete_acceptance | après contexte Runtime approuvé, attacher une sélection préparée au PreparedSubmission neuf ; reserve Accepted replay reste avant résolution ; Review/approve/resume ne refont pas le choix |
| service/mod.rs::VerifiedProfile::open_context / AuthenticatedBatchContext | transporte le contexte opérateur authentifié pour la préparation ; le binding de lancement des reprises vient du job, pas du registre courant |
| runner/broker.rs::PreparedSubmission / prepare_broker_submission / BrokerRunState | nouveau champ privé/sélection crate-private ; APIs et reconstruction RunPlan historiques conservées |
| broker/writer.rs::Command::AcceptPrepared | dispatch vers accept Legacy ou accept strict préparé ; ownership NotQueued/Unknown et replay actuels conservés |
| broker/journal.rs, types.rs | StoredBindings V1/V2, paire Accepted, publication/profile Full load, getters opaques et budget/inventaire |
| journal/confirmed.rs, journal/recovery.rs, checkpoint/validation.rs | mêmes métadonnées acceptées jusqu'aux révisions et STOP ; immutabilité supplémentaire sans toucher hashes d'autorité historiques |
| runner/managed.rs::BrokerPreparation/broker_prepare/prepare_authority/broker_runtime | seed séparé et RuntimeAuthority lié, handle launch pour start uniquement ; anciens callers prepare_authority gardent Legacy |
| runner/broker/executor.rs et executor/{journal.rs,helpers.rs} | retenir/recouper cap du job déjà chargé ; mêmes profils pour worker/Import/ImportVerification/FinalScan/Export, aucune nouvelle permission |
| runtime/launch_profile.rs (nouveau), runtime/{managed.rs}, runtime.rs | types/métadonnées/capacité, ajout de commande start-only et contrôle mémoire avant garde ; API publique existante conservée |
| broker/enforcer.rs et enforcer/client.rs | validation pure, digest complet déjà existant, identité attendue du contrôle et wrapper de garde ; restore reste ObserveOnly |

`runtime/invocation.rs` ne devrait pas nécessiter de changement fonctionnel : politique2/environment/fingerprint actuels préservés. `enforcer/server.rs` ne doit pas lire le profil pour ses acteurs STOP ; ne l'inclure que si des validations de métadonnées communes l'exigent. Pas de DB, schéma Store, transport, worker, service ou identité ajouté. Pas d'installation OS, opération cloud/PKI, commit/push ou moteur autorisé par ce contrat seul.

## 8. Plan vérifiable et preuves proposées (toutes NOT_RUN)

1. **Format/acceptation** : fixtures publiques golden V1 intactes ; parser strict V2/couplage/version/domaines, golden indépendant UN NUL et variante double NUL refusée ; création réelle FS du fichier exact et durabilité avant Accepted. Refus champs inconnus, traversal/ref externe, metadata libre, scope/parent/intent/endpoint/invocation divergents. Directory fermé : extra fichier/sous-directory/nom inconnu refusé avant start, compté metadata-only (ou inconnu explicite), contenu jamais ouvert/réparé ; partial avant manifest/Accepted conservé et compté. Defaults RegistryV1 Legacy et RegistryV2 obligatoire/Podman refusé. Cibles voisines réelles : broker_submission, broker_journal, broker_checkpoint, unités service.
2. **Review/replay/reprise et seed** : vrai Journal/Writer, change registry après Review, mêmes bytes/ref/engagement sur replay/reopen ; jamais nouvelle préparation/profile sous même opération ; erreur partielle/no ACK conservatrice. Deux jobs et deux scopes partageant config_dir et mêmes autres valeurs ont des seeds distincts ; échange du manifest/seed autorité refusé ; legacy RuntimeAuthority None et invoc.policy2/goldens identiques. Pas de blob fournisseur ni configDocker réel.
3. **FS et STOP** : vrais fichiers privés synthétiques ; mode/UID/ACL/link/bytes/parent remplacé/absence refusés avant start ; liens DB connus refusés avant open par exclusions actuelles. D'un côté, altérer manifest/lien Accepted/chaîne/profil engagé refuse toute nouvelle projection. De l'autre, exercer séparément blob manquant, bytes/digest modifiés, mode/ACL/lien défaillant, parent launch absent/substitué/non admissible et extras : reader préserve les mêmes Arm/deadlines/CID prouvés sans ouvrir/recréer le blob. Pour chaque variante, vérifier append_cancel : ACK seulement si contrôle/audit/quota/budget restent prouvés, sinon erreur typée et écart explicite avec enforcement conservé ; pas de PASS fonctionnel universel de STOP tant que les variantes de budget inconnu restent ouvertes. RuntimeAuthority reopen/stop sur fixture CLI Rust fonctionne sans le profil ; aucun start après restore.
4. **Transport** : fixtures CLI Rust explicitement approuvées des cibles runtime_prearmed/runtime_managed/runtime_program ; argv contient un seul --config au bon endroit, exacts bytes {}, aucun fallback à une config synthétique historique porteuse de canaris ; programme et contexte originaux intacts. Worker et chaque purpose helper utilisent le binding du parent malgré IDs distincts ; mauvais/missing profil ou contrôle Legacy refuse avant spawn ; garde consommée une seule fois, aucune préparation IO/await déplacée après elle. Ajouter la régression de validation pure/Journal réel : helper préparé puis owned/Arm AVANT worker Created (owned principal absent ; context.worker conserve le vrai Worker Reserved/Preparing requis par ce parcours), profil commun correct admis, None/autre scope/parent/ref/invocation refusé. Le recoupement commun n'est jamais conditionné à ces options ; ne pas construire un faux parcours qui retire un worker requis pour tester autre chose. L'absence d'autorité au seul rev0 reste admise. Pas de moteur/réseau/fournisseur réel dans cette phase.
5. Après revue indépendante et résultats Luna/stricts, Root choisit un essai CLI réel épinglé et borné contre un serveur Rust de qualification puis la recette moteur autorisée, preuves séparées. Ces essais ne sont pas exécutés ni autorisés par cette note. Aucun PASS D5 ou G09 déduit des fixtures.

## 9. Décisions à valider par Root avant code

1. Approuver la paire bindings V2 + Accepted version2, ExecutionIntent/credentials/plans inchangés, et les champs/noms/domaines proposés avec **journal_scope_hash**.
2. Approuver RegistryV2 avec choix obligatoire et mode Legacy explicite en plus de DockerMinimalV1, plutôt qu'une conversion silencieuse des V1.
3. Approuver le stockage job/docker-launch-v1/config.json, le seed namespace/version2 et la portée de l'immutabilité FD actuelle (sans nouvelle promesse d'inode persistant/scellement).
4. Approuver la couture privée PrearmedStartControl/wrapper guard préservant la signature publique CID, et la règle stricte Some interdit aux starts Legacy ; STOP/restore ne dépendent jamais du blob.

Les quatre décisions ci-dessus ont été approuvées par Root, sous les quatre précisions de revue intégrées ici. Root confirme le refus conservateur de l'ACK append lorsque budget/audit sont inconnus, avec projections/enforcer prouvés préservés ; aucune solution de contournement n'est incluse ni aucune obligation d'audit/quota/contrôle réduite.

## 10. Premier RED choisi, à écrire seulement après le dernier GO

Test unitaire unique proposé dans `src/broker/service/profile.rs`, module enfant `tests`, nom exact `broker::service::profile::tests::registry_v2_accepts_explicit_minimal_docker_start_profile`. Il emploie les **API existantes** `Registry::load(path)` puis `registry.profile(id)` ; aucun nouveau type/fonction requis pour compiler son RED.

Fixture : nouveau TempDir privé0700, JSON registryV2 complet composé des champs opérateur actuels avec valeurs synthétiques fixes (gateway loopback non appelé, Docker, endpoint Unix non ouvert, managed_clients=false, providers/manual=null, source/export roots distincts), et `prearmed_start_profile={version:1,mode:docker_minimal}`. Les paths de token/programme/config ne sont que références synthétiques normalisées, aucun de ces fichiers n'est ouvert/créé. Seul le fichier JSON de registre neuf est créé0600 et synchronisé par les vraies primitives FS privées. `Registry::load` actuel lit ce fichier et rejette le nouveau champ/version ; le test attend une lecture réussie et le profil Docker identifié. L'assertion positive doit donc échouer **après** la vraie ouverture/lecture/parsing du JSON, pas à la compilation ni sur un prérequis FS manquant. Les erreurs/messages restent fermés sans path/JSON/credential.

Portée de ce RED : acceptation de la nouvelle forme de configuration, pas disponibilité du programme/moteur, auth, construction du profil durable ou lancement. Les refus et autres parcours du plan suivront séparément. Luna exécutera une seule fois la cible exacte après gel et fin du build courant ; aucune commande Cargo/test lancée par l'implémenteur. Aucun source édité avant le GO Root.

Statuts : PASS pour cartographie source et mise à jour de ce contrat ; NOT_RUN pour toute implémentation/test/qualification du profil. Écart ouvert sur append STOP dans les variantes dont le budget devient inconnu avec les primitives actuelles. G09/D5 et autres gates conservent leurs statuts ; aucun code source modifié. Attente du dernier GO Root pour l'implémentation.
