# Contrat runtime concret — proposition et lot P1 approuvé

29 octobre 2026. Référence : `/tmp/ai-guard-worker-runtime-design.md`, code courant `src/runtime.rs` et `src/runner.rs`. Première portée mono-hôte ; worker = instance d'une commande, terminale ensuite. Docker/Podman uniquement ; aucun claim multi-hôtes/HA/cloud/Kubernetes. Aucune nouvelle infrastructure ni langage. Le présent fichier est hors dépôt.

## 1. Petit lot P1 approuvé par root : résultat détenu avant suppression

Interfaces convenues avec l'intégration runner :

```rust
pub struct RetainedExecution {
    result: ExecutionResult,
    instance: CreatedInstance, // privé : create acquitté ET référence retournée valide
}
impl RetainedExecution {
    pub fn result(&self) -> &ExecutionResult;
    pub fn container_name(&self) -> &str;
}
impl Runtime {
    pub async fn execute_retained<F, Fut>(
        &self, spec: &ContainerSpec, input: Option<File>,
        duration: Duration, stdout_limit: usize, on_started: F,
    ) -> anyhow::Result<RetainedExecution>
    where F: FnMut(RuntimeState) -> Fut,
          Fut: Future<Output = anyhow::Result<()>>;
    pub async fn cleanup(&self, execution: &RetainedExecution) -> anyhow::Result<()>;
}
```

`RetainedExecution` n'a aucun Drop destructif. Le runner prépare et engage son événement terminal à partir de `retained.result()`, puis appelle cleanup. `?` lors de la finalisation laisse l'instance terminale inspectable. Un échec de rm laisse le résultat détenu accessible ; il n'est pas remplacé par un faux résultat métier ni par une durée zéro. Root prévoit un statut cleanup removed/retained distinct dans RunSummary et un code CLI non nul si retained.

En erreur après create acquitté : arrêt ciblé si le conteneur peut tourner, inspection si disponible, instance conservée et erreur indiquant si l'arrêt est inconnu. Aucun faux terminal pour remplacer l'erreur. Un échec de create ne confère aucun token : aucune adoption, kill ou rm d'une instance préexistante. L'ancien execute reste un wrapper sur le même noyau, avec cleanup automatique pour ses usages helpers/tests ; aucun second corps de boucle start/attach/timeout.

Identité privée du lot : conserver le nom généré par ContainerSpec et la référence réelle renvoyée par create. Les commandes après create ciblent cette référence, pas un nouveau nom. Si la réponse create est inutilisable, aucun token n'est attribué : ne pas lancer, inspecter, kill ou rm par le nom ; conserver le nom uniquement pour diagnostic et état/ownership inconnus. Cette règle a été reproduite/corrigée pendant la revue du lot P1, y compris succès CLI avec sortie vide/malformée/tronquée et nom réaffecté. Une réponse create échouée/ambiguë ne justifie jamais une adoption par nom.

Le lot P1 ne livre pas encore la réservation, la consommation durable de start, l'ID d'autorité moteur ni une réconciliation après crash. Tests requis : ordre terminal-ack avant rm, finalisation/audit refusé conserve inspectable, rm échoué conserve résultat, conflit create sans kill/rm, callback start refusé provoque arrêt ciblé et conservation, moteur indisponible rend arrêt inconnu. Fixtures Rust puis preuves HTTP/CLI et moteur séparées. Runtime/tests retention à l'agent runtime ; runner/intégration/opérations/audit projection à root.

## 2. Contrat minimal suivant pour le registre — à valider avant code

Ne pas refaire une hiérarchie d'adaptateurs : conserver RuntimeKind, ContainerPurpose, create_args et les paramètres sûrs existants. Ajouter une construction stable du spec, des primitives ciblées et un snapshot enrichi. Les constructeurs DTO de contrôle appartiennent au contrat root, pas au runtime.

```rust
pub struct AttemptKey {
    pub authority_id: Uuid,
    pub session_id: Uuid,
    pub operation_id: Uuid, // créé et durable AVANT create, jamais régénéré en reprise
}
pub struct ContainerHandle {
    key: AttemptKey,
    id: String,          // référence réelle validée du runtime
    name: String,
}
pub struct CpuQuota { pub quota: u64, pub period: u64 } // ratio exact de cores
pub struct ObservedAllocation {
    pub cpu: Option<CpuQuota>,
    pub memory_bytes: Option<u64>,
    pub pids_limit: Option<u64>,
}
pub struct ProcessObservation {
    pub status: String, pub running: bool,
    pub started_at: Option<DateTime<FixedOffset>>,
    pub finished_at: Option<DateTime<FixedOffset>>,
    pub exit_code: Option<u32>, // absent/sentinelle négative = None, jamais zéro
    pub oom_killed: Option<bool>,
}
pub struct ContainerObservation {
    pub handle: ContainerHandle,
    pub image_id: String,
    pub restart_count: Option<u64>,
    pub process: ProcessObservation,
    pub allocation: ObservedAllocation,
    pub observed_at: DateTime<Utc>,
}
```

Ces Option signifient « preuve insuffisante » ; zéro ou une limite illimitée ne sont pas une allocation bornée connue. Conserver aussi un diagnostic typé pour distinguer champ absent, sentinelle, illimité et valeur invalide, sans persister le JSON inspect complet. Le registre exige image/identité/ressources connues et conformes ; sinon il conserve quota/état à réconcilier. Un snapshot partiel ne libère rien.

Fonctions proposées, chacune une opération de CLI bornée :

```rust
ContainerSpec::with_operation(session, purpose, image, argv, network, operation)
    -> Result<ContainerSpec>; // name reste identique après sérialisation/reprise
Runtime::create(&ContainerSpec) -> Result<ContainerHandle>;
Runtime::observe(&ContainerHandle) -> Result<ContainerObservation>;
Runtime::lookup_created(&AttemptKey, &ContainerSpec)
    -> Result<Option<ContainerObservation>>;
Runtime::start_once(&mut AttemptGuard, FreshStartPermit, &ContainerHandle, ...)
    -> Result<RetainedExecution>;
Runtime::attach_existing(&ContainerHandle, ...) -> Result<RetainedExecution>;
Runtime::stop_owned(&ContainerHandle) -> Result<ContainerObservation>;
Runtime::cleanup(&RetainedExecution) -> Result<()>;
```

`start_once` conserve `start --attach` pour le premier lancement et sa capture de sortie actuelle. `attach_existing` ne contient **jamais** start ; une reprise d'un terminal inspecte/finalise plutôt qu'attacher. `create` capture l'ID réel, observe et vérifie l'identité avant start. Les commandes start/inspect/kill/rm post-create utilisent l'ID complet ; le nom n'est qu'un moyen de retrouver une création dont la réponse a été perdue. Ne pas adopter sur seuls labels : comparer tentative réservée, nom, image immutable, argv/workdir, mount du volume et restrictions attendues. Collision non prouvée : conflit et conservation, sans rm.

`lookup_created` retourne None uniquement après un inventaire ciblé réussi dans l'autorité vérifiée, pas à partir d'un exit non nul quelconque de inspect. Cela signifie « absent à cette observation », **jamais preuve terminale ou autorisation de relancer**. Timeout, permission, JSON malformé et changement moteur sont des erreurs d'observation.

## 3. CLI inspect : données sélectives, ressources exactes

Lire l'inspection JSON bornée d'un ID, puis désérialiser seulement ID, image, State, RestartCount, HostConfig et mounts nécessaires. Ne pas logger/persister Config.Env ni le document complet : il peut contenir les secrets provider. Conserver les bornes de capture actuelles et refuser une réponse tronquée. Les erreurs publiques n'incluent pas le JSON brut.

Normaliser CPU par ratio entier : NanoCpus positif devient nano / 1e9 ; sinon quota/period positifs ; si les deux existent, vérifier leur cohérence. Ne pas dépendre uniquement de NanoCpus : Podman ne le calcule que pour CpuPeriod=100000. [Source officielle v5.7.1](https://github.com/containers/podman/blob/v5.7.1/libpod/container_inspect.go#L540-L546). Memory et PidsLimit positifs sont des bornes configurées observées ; absence/illimité ne sont pas zéro. Ces observations sont la **configuration runtime**, pas CPU/RAM consommés ; effets cgroup vérifiés séparément sur moteur réel.

La conversion terminale exige état terminal reconnu par kind (Docker exited ; Podman exited/stopped), Running=false, code observé non négatif, dates valides/ordonnées, restart_count=0, image/ID/limites inchangés. [États Podman](https://github.com/containers/podman/blob/v5.7.1/libpod/define/containerstate.go#L21-L28), [sentinelle inconnue](https://github.com/containers/podman/blob/v5.7.1/libpod/container_internal.go#L1404-L1412). Un job rapide déjà terminal à la première observation conserve un démarrage historique ; il ne devient pas un Running courant fictif.

Mesure décidée par root : processus StartedAt→FinishedAt avec CPU/RAM configurés inspectés, billable=false ; restart non nul, horloge/ressource/identité incohérente => mesure inconnue, quota retenu. ExecutionResult.duration_ms reste le temps attach/contrôle et n'entre pas dans le ledger.

## 4. Autorité locale et garde empêchant une émission tardive

`Runtime::new(kind)` ne suffit pas à identifier une autorité après changement de contexte. Ajouter `AuthorityBinding { authority_id, kind, target_fingerprint, engine_fingerprint }` dans le plan/réservation. `authority_id` est configuré par le contrôle, pas tiré d'un label. Figer l'invocation/endpoint et l'environnement de sélection du CLI pour toute la tentative ; ne pas relire un contexte mutable entre create et start sans contrôle. Récupérer un identifiant moteur natif s'il est réellement exposé, sinon marquer son absence : ne pas fabriquer un ID universel Podman avec le seul hostname/version.

Pour Docker Desktop, un endpoint Unix explicite et un fingerprint moteur contrôlé rendent cette liaison concrète. Pour Podman natif, il faut lier le compte et le stockage/configuration du moteur local ; pour un endpoint Podman distant/VM, l'endpoint sélectionné doit être explicite. Le mapping exact CLI/fingerprint de chaque mode doit avoir une fixture native et une preuve réelle ; ce document ne qualifie pas un mode distant. Aucun nouveau daemon Podman obligatoire ni service de supervision n'est introduit par la proposition.

```rust
pub struct AttemptGuard { /* File verrouillé, clé, non Clone, champs privés */ }
pub struct FreshStartPermit { /* tentative + spec_hash + reçu durable, non Clone */ }
AttemptGuard::try_acquire(trusted_lock_root, &AttemptKey) -> Result<AttemptGuard>;
```

Garde **par tentative**, pas verrou global tenu pendant toute la session : plusieurs workers distincts peuvent fonctionner en parallèle. Tous les brokers mono-hôte doivent utiliser le même répertoire d'autorité privé et canonique. Ouvrir le fichier de lock sans suivre lien/hardlink, vérifier son inode/permissions, ne jamais supprimer/remplacer un lock actif. `libc::flock(LOCK_EX|LOCK_NB)` est disponible via la dépendance déjà présente ; garder le descripteur du début de la reprise jusqu'à abandon/fin, pas seulement jusqu'au SELECT SQL. Un processus suspendu garde son verrou ; aucun takeover par TTL/lease SQL.

FreshStartPermit ne peut être créé que depuis la **première** consommation durable accordée par le contrôle pour cette tentative. Il est consommé par valeur à l'unique émission start. Un replay serveur AlreadyConsumed produit seulement inspection/reprise ; jamais un nouveau FreshStartPermit. Le marqueur durable reste consommé après crash et même si un fils CLI peut encore achever son ancien appel. La nouvelle autorité ne réémet pas start et ne libère pas le quota sur une absence passagère. Le verrou local résout la concurrence des processus vivants ; la consommation durable résout la reprise après leur mort. Ni l'un ni une lease ne fence un broker arbitraire sur un autre hôte.

L'annulation NeverStarted après consommation ne doit pas être automatisée sur « StartedAt vide » ou « ID absent » : un ancien appel moteur peut être en vol. Ce cas garde son quota et nécessite une procédure de réconciliation sûre. Aucune promesse de liveness/exactly-once complète n'est tirée de la garde ; le produit doit montrer le blocage et l'action opérateur, pas démarrer un remplaçant caché.

## 5. Compatibilité et modifications minimales

- Garder RuntimeState et la validation stricte déjà corrigée pour les appels existants ; enrichir l'observation worker sans remettre un default ExitCode/Running. Centraliser les règles terminales, pas deux interprétations contradictoires.
- ContainerSpec::new reste utile pour opérations nouvelles, surtout helpers ; le runner worker durable utilisera exclusivement with_operation avec l'operation persistée. Ne pas casser l'unicité de deux exports d'une même session : chaque export reste une opération distincte.
- create_args reste la source des restrictions et reçoit les mêmes ressources typées que la réservation ; pas de deuxième liste d'arguments driftante.
- Import/export restent helpers de contrôle à commandes fixes, sans slot client séparé ni credentials métier/contrôle, mais autorisés pour le bon tenant/volume et bornés en concurrence. Garder le wrapper execute pour leur comportement actuel ; la commande métier passe par retained puis durable finalisation.
- RunPlan ajoute version, operation stable, réservation/receipt, binding d'autorité et handle réel acquitté. Le fichier local est un checkpoint, pas un remplaçant de la base tenant. Runner, control DTO/API et SQL sont aux propriétaires root/core ; primitives runtime et tests de celles-ci à l'agent runtime.
- Cleanup inconnu/échoué reste séparé de la mesure terminale. Aucun changement de stockage, infrastructure, nom de tenant ou rôle worker à déduire du runtime.

## 6. Régressions avant intégration registre

1. P1 retained : prouver ordre HTTP terminal-ack avant rm et preuve conservée sur 503 ; rm échoué garde code/temps connus ; conflit create ne donne aucun droit ; callback refusé arrête uniquement son ID et garde instance ; moteur absent rend arrêt inconnu.
2. Stable operation/name après reopen ; stdout create → ID conservé et tous les contrôles post-create ciblent l'ID ; aucune nouvelle création sur replay, collision non adoptée/supprimée.
3. Snapshots Docker/Podman natifs : champs manquants, code -1, stopped, état running incohérent, restart_count, image modifiée, limites illimitées/manquantes, quotas CPU avec période différente, champs Env ignorés et jamais persistés/loggés.
4. Deux vrais processus, même tentative : détenteur suspendu garde lock ; second échoue, TTL n'accorde rien ; après mort, lock acquis mais StartIssued reste non rejouable. Autres tentatives restent parallèles. Remplacement/symlink/hardlink de lock refusés.
5. Vrai moteur après module : crash contrôlé avant/après create/start/terminal commit, commandes très courtes, plusieurs observateurs, effet-marqueur présent une seule fois, quota retenu sur inconnu, preuve terminale avant rm. Docker/Podman et hôtes qualifiés séparément ; aucune fixture ne qualifie Podman réel.

Les seules décisions encore nécessaires pour le lot registre sont le DTO d'autorisation lié au spec, le mapping concret d'autorité pour les modes Podman choisis et la procédure opérateur d'incertitude. Les décisions root mono-hôte / commande terminale / dates processus / restart inconnu sont intégrées ci-dessus.
