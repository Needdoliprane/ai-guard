# Port privé exec du relais — segment logiciel

Ce segment implémente le transport d'octets dans
[`src/runtime/managed/relay.rs`](../../src/runtime/managed/relay.rs). Il n'ouvre
aucun parcours produit : `RelayExecPermit` n'a pas de constructeur production.
Le raccord doit encore recevoir une admission courante liée à l'audit durable,
au binding workload et au registre gardant le `RoleLease` exclusif. Une fixture
logicielle n'atteste pas ces propriétés. Le [handoff approuvé](RELAY_RUNTIME_HANDOFF.md)
conserve l'autorité d'architecture ; [English](RELAY_EXEC_PORT.en.md).

## Contrat d'intégration

- Le propriétaire de confiance consomme un permis distinct de start. Il ne
  reçoit aucun CID, argv ou identité depuis une frame du worker.
- Le permis retient `OwnedContainerSnapshot`, spécification autorisée, moteur,
  invocation et profil. Avant spawn, le port vérifie ID complet, session,
  opération, image immuable, configuration v2/v3, allocations, Running,
  restart zéro et `StartedAt` positif identique. Il inspecte les restrictions
  de la spécification, dont `network:none`, puis revérifie invocation ancrée et
  profil retenu sans attente supplémentaire avant spawn. L'inspection et la
  création de processus ne forment pas une transaction moteur atomique.
- Docker exige le profil privé minimal retenu ; Podman exige le binding de
  stockage/marqueur et la sélection d'environnement retenue. Les commandes
  start et STOP existantes ne changent pas. Kubernetes n'a aucun port exec
  implémenté et ne peut pas être sélectionné via cette API Docker/Podman.
- `io_pair()` prête des wrappers `AsyncRead`/`AsyncWrite`, compatibles avec
  `ControlOwner::serve`. Aucun FD, child ou stderr ne quitte le propriétaire.
  Le propriétaire contrôle framing, capacité et délais absolus/inactifs 5/2 s,
  et appelle `close_and_join()` après retour ou annulation du service. Le port
  runtime ne lit jamais stdout en parallèle et ne crée aucun codec concurrent.
  `shutdown()` du writer ferme le pipe stdin réellement retenu, sans fermer
  stdout : le `shutdown` natif de `ChildStdin` Unix est sans effet sur son FD.
  Le test echo attend donc un vrai EOF puis les derniers octets du child.
- Le registre doit retenir le transport et sa lease jusqu'à jonction confirmée.
  La fermeture coupe stdin et stdout, attend deux secondes, demande au besoin
  la mort de la CLI seulement, et borne la jonction totale à cinq secondes.
  Les handles non joints restent possédés pour une nouvelle attente explicite.
  Le destructeur est un secours fermé, pas une preuve de jonction.
- Même exit zéro de la CLI, EOF et jonction locale laissent `OutcomeUnknown`
  pour le helper moteur. La seule réconciliation fournie réinspecte le worker
  exact terminal, avec identité/configuration/`StartedAt` inchangés. Elle ne
  l'arrête ni ne le supprime ; elle ne commet aucun terminal, quota ou usage.
  Aucun canal neuf ne découle automatiquement de cette observation.

## Dialectes et environnement

L'exec appelle uniquement `/usr/local/bin/ai-guard-relay serve-v1`, UID/GID
`1000:1000`, stdin ouvert, TTY et privilèges explicitement désactivés. Podman
désactive aussi sa séquence de détachement configurable. Docker n'installe sa
détection de touches de détachement qu'en TTY. Ce sont des propriétés du code
et des sources consultées ; aucune version moteur installée n'est qualifiée.
Voir les manuels [Docker exec](https://docs.docker.com/reference/cli/docker/container/exec/)
et [Podman exec](https://docs.podman.io/en/latest/markdown/podman-exec.1.html),
puis les sources officielles [Docker hijack](https://github.com/docker/cli/blob/master/cli/command/container/hijack.go)
et [Podman exec](https://github.com/containers/podman/blob/main/cmd/podman/containers/exec.go).

Docker hérite l'environnement du conteneur. Le handoff fournisseur courant
place `AI_GUARD_PROVIDER_BOOTSTRAP` dans `Config.Env`, puis le bootstrap construit
l'environnement privé de Codex ou Claude. L'absence de `--env` ne serait donc
pas une garantie d'absence de credential dans le helper. Les noms connus sont
neutralisés par sept arguments constants `--env=NOM=` : bootstrap, clés
OpenAI/Codex/Anthropic, token OAuth Claude et répertoires de configuration
Codex/Claude. Aucune valeur de credential n'est incluse dans argv. Les sources
[Docker de remplacement d'environnement](https://github.com/moby/moby/blob/master/daemon/container/env.go)
et [Podman de parsing](https://github.com/containers/podman/blob/main/pkg/env/env.go)
conservent la valeur vide explicitement fournie.

L'inspection transitoire refuse les noms supplémentaires, doublons et valeurs
publiques non reconnues. Les valeurs fixes du Dockerfile, `TERM` fixe et
`container=podman` sont admises ; `NODE_VERSION` exige une version numérique
courte de majeure 24, `YARN_VERSION` de majeure 1. Ce profil ne qualifie pas
l'image : le futur émetteur doit attester le digest immuable contenant le helper
root-owned readonly. Les processus du même worker restent non fiables et
partagent son UID ; ce port n'isole pas le helper des fichiers fournisseur
déjà présents dans le même worker. Aucun nettoyage global du home n'est fait.

Stderr est drainé sans rétention brute ; seuls compteurs et codes fermés
subsistent. Toute activité stderr interrompt les IO du canal ; plus de 16 Kio
ferment son FD. Aucun message OS ou texte arbitraire n'entre dans les erreurs.

## Validation et limites

Les tests sous `managed/relay/tests.rs` couvrent arguments fermés, divergence
d'incarnation/configuration, environnement hérité, vrais pipes et enfants Rust
bornés, stderr opaque et mort/jonction de la CLI. L'entrée enfant ignorée est
uniquement appelée par les parents. Elle n'est pas comptée comme test produit.
Le résultat de l'exécution QA doit être consigné séparément : la présence de
ces tests et rustfmt ne vaut pas PASS.

Le gel67 a conservé un FAIL du parent echo : timeout à la lecture de fin,
après readiness réussie. La cause source est la délégation au `shutdown`
sans fermeture du pipe. Le correctif ferme ce FD via l'emprunt de son `Option`
dans le propriétaire ; aucun délai produit ni délai du test n'est augmenté.
Le gel73 passe ensuite ses sept parents dans QA AM, puis la compilation toutes
cibles dans QA AO. L'enfant ignoré n'est pas un PASS supplémentaire.
[Preuve attachée aux sources](evidence/2026-11-07/relay-runtime-exec/README.md).

Docker/Podman réels, topologies VM, helper dans une vraie image, credential
absent à l'exécution, arrêt distant et reprise sous registre durable restent
`NOT_RUN`/`BLOCKED` selon leurs dépendances. Aucun moteur, fournisseur, cluster,
service OS ou téléchargement n'est autorisé par ce segment. Aucun gate fermé.
