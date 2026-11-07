# Helper Rust du relais local au worker

[English](HELPER.md) · [Client HTTP](README.fr.md) · [Preuves datées](../../engineering/evidence/2026-11-07/relay-worker-helper/README.md)

Le [helper](../../../src/relay/channel/helper.rs) relie un socket Unix interne au
worker à un flux stdin/stdout dédié et cadré. Il ne reçoit ni preuve workload,
credential runner, sélection de tenant, URL gateway ou socket moteur.

Ce composant logiciel est livré ; le canal runtime n'est pas raccordé.
Le propriétaire d'exec broker, les contrôles d'admission/binding runtime et
l'outbox durable de la [direction approuvée](../../engineering/RELAY_RUNTIME_HANDOFF.md)
ne sont pas raccordés à ce composant. Conserver les workers en `network:none`,
hors du réseau de contrôle. Ne pas ajouter de proxy TCP, de montage de socket
hôte ou de transmission manuelle de credential pour contourner cette intégration
absente. La campagne réelle de transport Docker/Podman et la recette A1/A2/B1
restent **NOT_RUN** pour ce composant.

## Image et invocation

Le [Dockerfile worker](../../../images/worker.Dockerfile) construit
`ai-guard-relay` avec la toolchain Rust épinglée et le copie dans
`/usr/local/bin/ai-guard-relay`, propriétaire root, mode `0555`. La racine readonly
relève du confinement runtime ; placer le binaire dans l'image ne le prouve pas.
Ne pas copier de remplacement dans `/workspace` ou `/home/agent`, ni adopter une
ancienne image sans ce helper.

Depuis la racine du dépôt, l'opérateur peut construire l'image avec les
permissions habituelles de son installation et les dépendances épinglées :

```sh
docker build --file images/worker.Dockerfile --tag ai-guard-worker:relay-local .
```

Cette commande est une instruction, pas un build exécuté ou qualifié par ce lot.
Le client construit et distribue ses images ; aucune image publique préconstruite
n'est exigée. Préparation hors ligne et build Podman nécessitent leurs propres
prérequis vérifiés.

Le seul mode interne est l'invocation fixe
`/usr/local/bin/ai-guard-relay serve-v1`. Ce n'est pas un CLI de credential
utilisateur. Stdin/stdout doivent être des pipes/sockets sans TTY ; autres
arguments, fichiers ordinaires et terminaux sont refusés. Le binaire utilise la
racine fixe `/home/agent/.ai-guard-relay`, sans chemin issu de l'environnement ou
des argv. Le futur propriétaire d'exec de confiance doit fournir et posséder ces
flux. Une invocation dans un shell interactif ne raccorde pas un worker.

Le helper ne lit pas la sortie fournisseur et ne découvre aucun credential dans
l'environnement. Conserver la preuve workload et sa référence privée dans le
contrôle de confiance. Credentials humains, runner et fournisseur ne deviennent
pas une autorité relais dans une frame.

## Publication et accès local

Le profil worker prévu fournit un tmpfs privé dans `/home/agent`. Inspecter
UID/GID, modes, image, montages, limites et réseau réels avant qualification
runtime ; ces tests logiciels n'inspectent pas un vrai worker ai-guard.

Le helper reçoit un `hello` V1 valide, renvoie exactement canal, epoch, limites
résolues et version helper dans `ready`, puis termine cette écriture avant de
créer son listener et de publier la découverte. Un flock du répertoire exclut
un autre helper utilisant la même racine retenue. Chaque canal crée un namespace
neuf :

| Entrée | Comportement |
|---|---|
| Répertoire de génération | Nouveau répertoire `0700` ; au plus 128 namespaces conservés |
| `s` | Nouveau socket Unix `0600` ; contrôles d'inode retenu, liens, UID peer et longueur de chemin |
| `relay-v1.json` | Nouveau record versionné immuable `0600` avec canal/epoch et faits de l'endpoint local ; aucun credential |

Aucun socket ou record existant n'est adopté, écrasé, réparé ou supprimé.
La fermeture du helper conserve ses entrées. Au plafond, préserver le namespace
pour investigation explicite. Supprimer des records ou changer d'epoch ne résout
pas une mutation inconnue.

L'API `discover(root, generation)` ouvre une seule génération explicite et
conserve le FD du record. `DiscoveredHelper::connect_checked(root)` revérifie
record et endpoint autour de la connexion. Elle ne recherche pas de génération
« latest » et ne réactive pas les anciennes. UID local et découverte sont des
contrôles de transport, pas des identités tenant, humaine ou agent authentifiées.

Les applications utilisent les frames V1 fermées `send`, `inbox` et `ack`, sans
champ URL, argv libre, tenant, identité source ou credential. Le helper vérifie
canal/epoch, correlation, opération et IDs des reçus de mutation. L'endpoint
d'inbox authentifié attendu reste vérifié par le contrôle ; le helper n'en
invente aucun. Le contenu de l'inbox est une donnée, pas une instruction ni un
ACK implicite.

## Bornes et fermeture

| Frontière | Limite implémentée |
|---|---|
| Handshake | 5 s au total |
| Assemblage/écriture de frame | 5 s absolues, 2 s d'inactivité après le début de progression |
| Opération en cours | 45 s absolues, transmission et réponse locale comprises |
| Frame requête / réponse encodée | 278528 / 2113536 octets, enveloppe comprise |
| Frame handshake, erreur ou close | 16384 octets |
| Payload métier | Bornes distinctes existantes des requêtes, corps, TTL, reçus et inbox ; l'enveloppe ne les augmente pas |
| Correlations | Un échange en cours ; au plus 1024 par epoch, lectures comprises |

Les longueurs sont contrôlées avant allocation du payload. Le codec partagé
refuse UTF-8 invalide, frames partielles, clés dupliquées récursivement,
profondeur excessive et champs/types/versions inconnus. Le lecteur async conserve
positions de préfixe/corps et délais lors des annulations de branches select.
Le contrôle est prioritaire, puis les IO/délais en cours, puis les acceptations
supplémentaires.

Aucune file ne conserve des corps de requêtes décodés. Pendant un échange,
les connexions supplémentaires sont fermées sans lire leur corps. Une seconde
frame sur la connexion active ferme ce client après au plus un octet de
préfixe, sans réponse busy corrélée ni second dispatch. Le premier échange reste
suivi jusqu'à sa réponse de contrôle, fermeture ou expiration.

EOF du contrôle ou un `close` correspondant ferme l'admission et les
clients/listener locaux possédés. L'unique propriétaire async ne crée ni tâche
détachée ni thread stdin bloquant. Ses FDs non bloquants possédés emploient le
contrat explicite `unsafe AsyncFd::register` de Tokio : le même FD valide reste
ouvert jusqu'à dérégistration/drop. Les
[preuves](../../engineering/evidence/2026-11-07/relay-worker-helper/README.md)
précisent l'API verrouillée et les tests.

Stdout ne porte que des frames. En cas d'erreur, le binaire émet au plus une ligne
fixe de 32 octets maximum sur stderr non bloquant, puis sort avec le code 126.
Les codes `protocol`, `timeout`, `unavailable`, `unsafe_filesystem` et `capacity`
sont préfixés par `relay_helper:`. Aucun corps, URL, credential ou erreur IO brute
n'est conservé. Plusieurs refus filesystem, dont le plafond de générations,
partagent `unsafe_filesystem` ; préserver les entrées plutôt que les réparer
automatiquement. Le futur broker doit encore posséder son drainage stderr borné
à 16 Kio et joindre son child ; ce helper ne fournit pas ce lifecycle.

Après transmission, perte ou timeout peuvent laisser un effet inconnu.
Conserver opération send/ACK exacte et contenu dans le contrôle de confiance.
Ne pas inventer de nouvel ID ni déduire d'EOF un rollback, la mort du worker,
la libération de quota ou un terminal observé. Rejeu durable, audit avant effet,
binding courant, grants/tenant et réconciliation restent les responsabilités du
contrôle.

Les onze tests helper emploient de vrais UDS locaux et un pipe OS dans une QA
Linux isolée. Ils couvrent notamment l'aller-retour requête inbox/erreur fermée,
fermeture, substitution, fragmentation et limites. Ils ne prouvent pas
send/page/ACK via HTTP, autorité workload ou runtime. Le canari EOF borné ne
réfute pas une starvation sous trafic continu. RSS, buffers agrégés du tenant,
Docker Desktop/VM Podman, Kubernetes, fournisseurs et gates production conservent
leurs qualifications distinctes.
