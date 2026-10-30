# ai-guard — tester le modèle cloud avec trois workers locaux

> **État rectifié du checkout :** les commandes et tests multi-workers décrits ici n'existaient pas dans les sources initiales. Ce texte est une proposition de recette, pas un résultat ni une procédure exécutable validée. L'avancement réel et les tests disponibles sont dans [docs/engineering/LOCAL_CLOUD_TESTS.md](docs/engineering/LOCAL_CLOUD_TESTS.md) et [VALIDATION.md](docs/engineering/VALIDATION.md).

## But et coût du scénario

La recette comporte **trois workers** : A1 et A2 pour `tenant-a`, B1 pour `tenant-b`. Elle ajoute un gateway et un dashboard, soit cinq conteneurs au total. Aucun compte fournisseur, cluster de cent pods ou paiement n'est nécessaire.

Cette topologie teste le modèle d'identité, les échanges autorisés, les données isolées, le quota et le relevé d'usage. Elle n'est pas une preuve de capacité à cent workers, de haute disponibilité ou de conformité d'un cluster Kubernetes.

Les trois workers sont des conteneurs de qualification maintenus en vie pour les échanges de test. Ils utilisent le binaire Rust ai-guard comme client de relais. Ils ne simulent ni l'activité d'un fournisseur IA ni une mesure de consommation réelle.

## Trois niveaux de preuve

| Niveau | Commande / environnement | Ce qu'il démontre |
|---|---|---|
| Contrats logiciels | Rust, HTTP local, SQLite réel | Authentification, portée, transactions, concurrence de réservation, grants et idempotence. |
| Trois conteneurs réels | `local_cloud_smoke`, Docker ou Podman choisi | Démarrage inspecté, volumes et secrets par rôle, relais depuis les workers et reprise du gateway. |
| Kubernetes futur | Cluster local avec CNI appliquant les policies | Adaptateur pod, identité et trafic réellement filtré par Kubernetes. Non livré dans cette version. |

## 1. Vérification logicielle sans moteur

À la racine :

```sh
CARGO_INCREMENTAL=0 cargo test --locked \
  --test tenant_storage --test tenant_api --test tenant_review \
  --test control_plane --test runtime_tenant --test runtime_local_cloud
```

Certains tests HTTP utilisent des ports loopback éphémères. Les tests de contrôle créent des bases temporaires et des identités de test. Le test de quota peut utiliser plusieurs connexions SQLite sans lancer plusieurs conteneurs.

## 2. Recette automatisée sur moteur réel

Prérequis : moteur Docker ou Podman fonctionnel, fournisseur Compose disponible, construction d'image autorisée, ports locaux 8787 et 4000 libres, espace disque et ressources suffisants. Le mode normal est rootless. Ne pas interrompre une installation existante pour récupérer ces ports.

Choisir un **nouveau dossier de configuration absolu hors du dépôt et du contexte de build**. Le test initialise ses secrets ; leur contenu n'est pas affiché. Exemple à adapter au poste :

```sh
AI_GUARD_TEST_RUNTIME=docker \
AI_GUARD_TEST_CONFIG=/chemin/absolu/hors-du-depot/ai-guard-recette \
CARGO_INCREMENTAL=0 cargo test --locked \
  --test local_cloud_smoke -- --ignored --nocapture
```

Remplacer `docker` par `podman` pour qualifier l'autre moteur. Une exception rootful déjà décidée par l'opérateur se signale explicitement avec `AI_GUARD_TEST_ALLOW_ROOTFUL=1` ; elle ne démontre pas un mode rootless et doit rester visible dans le rapport.

Le test effectue réellement : initialisation de la configuration, vérification du moteur, construction de l'image de contrôle, démarrage de gateway/dashboard dans un projet Compose au nom unique, réservation de deux capacités chez A et d'une chez B, puis démarrage des trois workers. Il inspecte `Running` et `StartedAt` avant de déclarer les workers actifs.

Il vérifie les labels d'identité, volumes distincts et montages de secrets par rôle ; il exerce le relais depuis les conteneurs, le quota, les refus, la révocation, l'usurpation, les mesures terminales et le redémarrage du gateway. Il ne doit pas présenter un timestamp construit par le test comme une observation de runtime.

Le nettoyage utilise le nom de projet unique du test et `compose down` sans `-v`. Les volumes et la configuration sont conservés pour inspection. Aucun `prune` global ni suppression des volumes d'autres projets. En cas d'arrêt brutal du processus de test, reprendre le nettoyage de ce seul projet identifié dans les sorties.

Un lancement sans moteur ne doit pas être compté comme PASS. La présence de `#[ignore]` évite de construire des images ou de démarrer des conteneurs lors de la suite logicielle ordinaire ; la sélection explicite ci-dessus est obligatoire.

## 3. Topologie consultable manuellement

La configuration est dans `deploy/local-cloud/compose.yaml`. Elle est distincte de la stack mono-client choisie par `ai-guard up`. Préparer un répertoire privé hors du dépôt :

```sh
./target/release/ai-guard init-local-cloud \
  --config-dir /chemin/absolu/hors-du-depot/ai-guard-lab
```

Cette commande crée les deux clients et leurs sept credentials distincts : runner/viewer de A et B, puis worker A1/A2/B1. Le registre ne contient que des références de fichiers ; les valeurs ne sont pas intégrées au JSON. Les originaux restent privés, avec projections destinées aux montages sous un parent privé.

Après construction de l'image par `ai-guard build --runtime docker`, on peut démarrer les services de contrôle :

```sh
AI_GUARD_CONFIG_DIR=/chemin/absolu/hors-du-depot/ai-guard-lab \
docker compose -f deploy/local-cloud/compose.yaml up -d gateway dashboard
```

Pour Podman, employer le fournisseur Compose du poste avec `podman compose`. Cette commande manuelle démarre uniquement gateway/dashboard : aucun worker n'est réservé ou lancé et le registre reste vide. Pour A1/A2/B1, utiliser la recette automatisée précédente, qui réserve les capacités **avant** création, inspecte le démarrage puis rapporte l'état. Ne pas démarrer toute la topologie avec un `up -d` sans cette étape de contrôle et ne pas remplir le registre de mesures inventées simplement pour obtenir une interface verte.

Le dashboard est exposé sur `http://127.0.0.1:4000` ; utiliser le viewer du client concerné dans l'IHM. Le gateway écoute sur `http://127.0.0.1:8787`. Chaque worker n'est relié qu'à son propre réseau interne et au gateway. Aucun runner/viewer de contrôle n'est monté dans les workers.

Pour conserver l'environnement après inspection, l'arrêt ciblé est :

```sh
AI_GUARD_CONFIG_DIR=/chemin/absolu/hors-du-depot/ai-guard-lab \
docker compose -f deploy/local-cloud/compose.yaml down
```

Les volumes sont conservés. L'utilisation de ports déjà occupés doit être résolue par une configuration explicite, sans arrêter des services inconnus.

## Matrice d'acceptation

| Scénario | Résultat nécessaire |
|---|---|
| A1→A2 sans grant | Refus et audit, aucun message accepté. |
| Grant A1→A2 sur `results` | Message durable uniquement sur ce canal et cette direction. |
| A2→A1 sans second grant | Refus. |
| A1→B1 | Refus, sans accès à la boîte de B. |
| Corps contenant un tenant/source usurpé | Rejet du DTO ou de la portée ; identité jamais remplacée. |
| Grant expiré ou révoqué | Nouvel envoi et relecture interdits ; aucun ancien message réactivé par un nouveau grant. |
| Troisième worker actif chez A, quota 2 | Réservation refusée de manière atomique. |
| Réponse perdue et même `message_id` rejoué | Même message, sans double acceptation ; autre contenu = conflit. |
| Arrêt confirmé et terminal rejoué | Usage compté une fois, y compris après redémarrage. |
| Durée/état runtime inconnus | Réconciliation requise ; aucune durée nulle inventée. |
| Panne de l'audit avant commit | Mutation annulée et opération refusée. |
| Viewer A : événements/SSE/export/usage | Aucune donnée B, y compris via IDs ou paramètres forcés. |
| Restriction réseau native | Vérifier aussi le trafic réellement refusé sur le moteur/CNI choisi, au-delà de la présence des réseaux dans un manifeste. |

Les scénarios de panne étendus et la qualification des restrictions runtime restent régis par [TEST_STRATEGY.md](TEST_STRATEGY.md). Le statut de chaque essai réellement exécuté est dans [../validation.md](../validation.md).

## Passage vers Kubernetes

Conserver A1/A2/B1 et les mêmes assertions applicatives. Remplacer seulement l'adaptateur et l'environnement de déploiement. Ajouter des tests de RBAC, service accounts, PVC, policies et trafic IPv4/IPv6/DNS. Un namespace ou un manifeste appliqué ne prouve pas l'isolement. Le [guide officiel NetworkPolicy](https://kubernetes.io/docs/concepts/services-networking/network-policies/) exige un plugin réseau qui implémente effectivement ces règles.

Un cluster local Kubernetes n'est pas démarré par la recette Compose et aucun résultat Kubernetes n'est revendiqué dans cette livraison.
