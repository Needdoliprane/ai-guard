# Publier la présence du broker de confiance

[English](PRESENCE.md) · [Installation du broker](README.fr.md) · [Identités workload et grants](../workloads/README.fr.md)

Le service Rust au premier plan peut publier sa propre présence authentifiée dans le registre brokers existant du tenant. Activer séparément chaque profil Registry wire2 avec `presence_file`. Les profils sans ce champ et Registry wire1 conservent leur comportement précédent. Cela publie une déclaration du broker et l'heure de réception du gateway ; cela n'inspecte pas les workers, ne prouve pas la santé runtime, ne libère pas de quota et n'établit pas une fédération multi-hôtes complète.

## Préparer l'autorité et le stockage privé

1. Utiliser un gateway de schéma6 ou7 et le namespace existant du tenant. Suivre la [migration explicite](../workloads/README.fr.md#préparer-le-stockage-sans-activer-lexécution) ; cela n'active pas la supervision.
2. Depuis le contrôle de confiance, [émettre un credential workload broker](../workloads/README.fr.md#préparer-et-enregistrer-une-opération-de-credential-par-cli) pour le broker et l'incarnation approuvés. Conserver son ID et son fichier de jeton privé. Puis [préparer et appliquer un grant broker](../workloads/README.fr.md#préparer-un-grant-ou-une-révocation-de-broker-par-cli) avec le runner de confiance pour les mêmes broker, credential et incarnation, avec **exactement une autorité** correspondant au libellé et au runtime de ce profil, le bail et l'expiration future voulus. L'API générale permet plusieurs autorités ; ce publisher exige la liste exacte d'une seule autorité. Le service ne crée ni ne renouvelle cette autorité.
3. Créer un **nouveau répertoire privé dédié à l'état**, hors des arbres projet/export, appartenant au compte du contrôle et en0700. Préserver un répertoire existant lors d'une reprise. Créer configuration et jeton dans des fichiers réguliers privés0600 avec parents privés0700 ; liens, ascendance dangereuse et substitutions sont refusés. Ne pas copier le runner dans le fichier du jeton workload.
4. Copier [presence.json](examples/presence.json) dans ce répertoire privé de configuration et remplacer chaque placeholder et chemin. C'est une entrée JSON typée, pas un dotenv ni une identité installée. Le jeton est référencé uniquement par son chemin ; jamais sa valeur dans le JSON, les arguments ou les logs.

Ajouter ce champ au profil voulu dans le [modèle Registry wire2 complet](README.fr.md#écrire-le-registre-opérateur-privé), en conservant ses paramètres existants :

```json
"presence_file": "/absolute/private/ai-guard/presence/config.json"
```

`broker serve --config /absolute/private/ai-guard/operator/registry.json` lit ce fichier au démarrage. Ses attentes gateway, tenant, namespace, installation, profil et runtime viennent de ce registre. Le gateway déduit le véritable tenant de la preuve workload authentifiée ; la configuration ne permet pas de choisir un autre tenant. Garder tous les éléments du contrôle hors des arbres source/export, même si le service installe aussi des exclusions de workspace et vérifie la capacité cumulée de protection des secrets avant d'ouvrir l'état de présence. Les limites existantes du service restent cumulées :64 entrées de fichiers protégés et64 secrets littéraux distincts (256Kio au total), autres profils et entrées du contrôle inclus.32 profils configurés ne garantissent pas la capacité pour32 publishers ; une configuration trop grande est refusée plutôt que tronquée.

| Champ présence V1 | Contrat |
|---|---|
| `version` | Exactement 1 ; champs inconnus refusés ; configuration ≤16 Kio. |
| `broker_id`, `credential_id`, `incarnation_id` | UUID canoniques non nuls correspondant au binding et au grant approuvés. Aucun remplacement généré après erreur. |
| `authority_id` | Libellé d'autorité approuvé, au plus 256 octets, sans caractère de contrôle. Le type de runtime vient du profil opérateur. |
| `broker_token_file` | Fichier privé absolu ; jeton de 32–4096 octets ASCII sans espace/caractère de contrôle, éventuellement suivi d'un LF. |
| `ca_bundle_file` | Fichier PEM privé absolu facultatif contenant seulement des certificats pour HTTPS, ≤64 Kio et 1–8 certificats. S'il est fourni, remplace les racines intégrées pour ce client. Ni clé privée d'AC ni contournement TLS. Omettre ou utiliser `null` pour les racines intégrées. |
| `state_dir` | Répertoire privé existant, mode `0700`, dédié à ce profil et périmètre. Conserve propriété, ancre, opérations en attente et reçus. |
| `interval_seconds` | Défaut 30 ; intervalle 5–150. Le double de l'intervalle ne doit pas dépasser le bail du grant. |
| `max_operations` | Défaut 8192 ; intervalle 1–32768 opérations retenues. Borne les nouvelles opérations ; une opération en attente déjà comptée exige encore la place de son reçu. |
| `max_state_bytes` | Défaut 67108864 (64 Mio) ; positif et au plus 134217728 (128 Mio). Inclut les preuves du flux et la capacité de reçu réservée avant POST. |

Le client HTTP accepte HTTPS ou **HTTP sur loopback numérique** pour un usage local délibéré. Il utilise seulement les routes fixes identity/self/heartbeat, désactive proxies ambiants, redirections et retries transparents, et borne réponses et délais. Un nom résolu vers loopback n'est pas un loopback numérique. Jeton et éventuel fichier d'AC sont retenus et revalidés ; modifier les fichiers vivants n'est pas un rechargement à chaud.

## Démarrer, observer et arrêter

Démarrer la même [commande broker au premier plan](README.fr.md#démarrer-le-service-au-premier-plan). Le service acquiert son rôle local et son listener avant de démarrer le publisher qu'il possède. Aucun job, opération moteur ou appel modèle n'est nécessaire pour publier la présence. Le publisher authentifie son périmètre, lit la vue broker courante, enregistre durablement son opération exacte avant POST, puis conserve le reçu validé. Une réponse perdue fait rejouer la même opération ; elle n'invente pas une nouvelle séquence.

Lire l'entrée obtenue avec `GET /api/v1/brokers` technique et une preuve runner/viewer, `GET /api/v1/brokers/self` et la preuve Broker pour sa propre entrée, ou la [vue nominative Brokers du dashboard](README.fr.md#lire-les-brokers-enregistrés-après-migration7) avec permission explicite `read_broker_registry` et schéma7. Vérifier autorité, incarnation, heure de réception, bail et fraîcheur. Le heartbeat est `declared` ; sa réception par le gateway est `observed` ; `runtime_inventory_complete` reste faux. Après arrêt, une ancienne entrée peut rester courante jusqu'à expiration du bail. Ni une entrée courante ni une entrée périmée ne prouvent l'état d'un worker.

L'arrêt normal du service signale et rejoint les tâches de publication avant de libérer son rôle local. `--recovery-only` n'ouvre ni configuration ni état de présence et ne publie aucun heartbeat ; son contrat existant de reprise des arrêts est conservé. Cela ne qualifie ni coupure électrique, ni service OS, ni échéance runtime autonome.

## Diagnostiquer et reprendre sans effacer les preuves

| Condition | Comportement et action opérateur |
|---|---|
| Transport ou indisponibilité temporaire générique | Tentative initiale puis au plus trois retries, avec attentes de 100, 250 et 500 millisecondes. L'épuisement arrête ce publisher. Diagnostiquer la connectivité puis redémarrer explicitement avec les mêmes état et autorité. |
| Refus d'authentification, grant, révision, horloge, protocole ou capacité | Arrête le publisher sans retries périodiques automatiques. Préserver configuration, opération en attente, reçus et preuves du contrôle ; réparer la cause exacte avant redémarrage explicite. Aucun repli runner, regrant ou renouvellement de credential. |
| Corruption du flux local, preuves historiques manquantes ou budget du flux épuisé | Préserver les preuves ; le publisher s'arrête ou est ignoré au démarrage. Cela seul ne désactive pas l'admission normale des jobs du broker. Ne pas réinitialiser le flux pour contourner la réconciliation. |
| Configuration privée, credential, AC ou répertoire retenu dangereux, absent ou changé | Refuse le démarrage ou ferme l'admission de nouveaux travaux métier. Un défaut de publication n'excuse pas des éléments de contrôle dangereux. |

Un état entièrement acquitté peut reprendre après rotation explicite du credential seulement avec les mêmes identité de credential, sujet et grant et l'historique heartbeat exact attendu. Une opération en attente ne traverse pas un grant changé/révoqué ni un changement de révision de binding. Pour un nouveau grant délibéré avec nouvelle incarnation approuvée et nouveau binding correspondant, préparer configuration et répertoire d'état neufs tout en conservant les anciens ; l'ancienne ancre immuable ne peut pas être réaffectée. Le serveur conserve la révision globale du heartbeat et les anciens faits. Un état manquant alors qu'un heartbeat existe pour cette incarnation exige une réconciliation ; ne pas créer de répertoire vide, UUID ou ancre de remplacement pour la contourner.

Les limites locales et les [limites cumulées d'audit workload](../workloads/README.fr.md#fixer-un-budget-daudit-workload-borné) s'appliquent ensemble. 8192 opérations à un intervalle nominal de 30 secondes correspondent à environ 68 heures 16 minutes ; à 5 secondes, environ 11 heures 23 minutes. Ces calculs excluent le temps HTTP/filesystem et ne bornent pas la durée murale : l'attente d'intervalle suit chaque cycle. Limite en octets, autre occupation d'audit ou expiration du grant peuvent arrêter la publication avant. `429 audit_workload_budget_exhausted` exige une action opérateur ; redémarrage, rotation et nouvelle incarnation ne réinitialisent pas l'usage d'audit cumulé du broker. Aucune procédure d'archivage/purge qualifiée n'est fournie. Préserver l'état plutôt qu'effacer les preuves pour la capacité.

L'implémentation vérifie et synchronise entièrement l'historique trois fois pendant un cycle réussi sans retry créant une nouvelle opération. Un snapshot caché évite un scan supplémentaire ; les contrôles complets avant POST, à l'acquittement et à la réouverture restent obligatoires. Une altération historique peut être détectée après les lectures identity/self ; le cache n'autorise aucun nouveau POST. Le coût augmente encore avec cet historique ; capacité soutenue et horizon de rétention par défaut restent à mesurer. Ce guide ne promet pas une publication autonome indéfinie. Lire [validation et limites restantes](../../engineering/VALIDATION.md) avant de choisir une politique de rétention opérationnelle.
