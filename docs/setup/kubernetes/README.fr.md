# Préparer la plateforme Kubernetes Docker Desktop pour ai-guard

[English](README.md) · [Français](README.fr.md) · [Index de configuration](../README.fr.md) · [Installation Docker/Podman](../local/README.fr.md)

Préparer et vérifier une plateforme Kubernetes de base à **un seul nœud** pour ai-guard sur le poste. Vérifier sa configuration et sa santé avec les commandes d'inspection ci-dessous. Elles sélectionnent explicitement le contexte `docker-desktop` et conservent le contexte global.

Ce guide couvre préparation du cluster, accès, ressources, images et diagnostic. Suivre l'[installation locale](../local/README.fr.md) pour installer ai-guard, lancer les sessions d'agents Docker/Podman et utiliser le broker.

## Préparer Docker Desktop et kubectl

Installer Docker Desktop selon le guide officiel pour [macOS](https://docs.docker.com/desktop/setup/install/mac-install/), [Windows](https://docs.docker.com/desktop/setup/install/windows-install/) ou [Linux](https://docs.docker.com/desktop/setup/install/linux/). Conserver installations, configuration et données existantes. Préparer les téléchargements d'images nécessaires via le réseau ou registre approuvé avant un usage hors ligne.

Ouvrir la vue Kubernetes de Docker Desktop. Si la création est déjà en cours, la laisser se terminer. Pour un cluster neuf, sélectionner la configuration de base à un nœud ; Docker décrit `kubeadm` comme son provisionneur à un nœud. Relever le provisionneur et la version Kubernetes proposés par la version de Desktop installée. [Instructions Kubernetes de Docker Desktop](https://docs.docker.com/desktop/use-desktop/kubernetes/).

Disposer de `kubectl` via Docker Desktop ou la [procédure officielle Kubernetes](https://kubernetes.io/docs/tasks/tools/). Résoudre délibérément les installations concurrentes, avec un client compatible avec la version du serveur.

## Vérifier le contexte et TLS

Examiner uniquement le contexte nommé et ses métadonnées publiques de connexion :

```sh
kubectl --context docker-desktop config get-contexts docker-desktop
kubectl --context docker-desktop config view --minify \
  -o jsonpath='{.clusters[0].cluster.server}{"\n"}{.clusters[0].cluster.insecure-skip-tls-verify}{"\n"}'
```

La deuxième commande affiche l'URL serveur puis le réglage de contournement de la vérification TLS. Exiger une URL HTTPS correspondant à l'installation Docker Desktop et un réglage absent ou `false`. S'arrêter si le contexte vise une autre installation ou si le réglage vaut `true` ; réparer le contexte Desktop selon la procédure d'administration de l'installation. Garder kubeconfig, credentials et clés privées confidentiels. Ces commandes filtrées n'affichent ni token ni certificat. Voir la [sélection du contexte](https://kubernetes.io/docs/concepts/configuration/organize-cluster-access-kubeconfig/) et la [sortie filtrée de configuration](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_config/kubectl_config_view/).

Contacter ensuite l'API sélectionnée en conservant la vérification TLS :

```sh
kubectl --context docker-desktop --request-timeout=10s version --output=yaml
```

Exiger une réponse serveur réussie. Le nom du contexte ou une liste de configuration seuls ne vérifient ni connexion ni confiance.

## Vérifier le nœud unique et ses ressources

Lister d'abord les nœuds. Continuer uniquement si la liste contient exactement un nœud appartenant à cette installation, puis attendre sa condition Ready :

```sh
kubectl --context docker-desktop --request-timeout=10s get nodes -o wide
kubectl --context docker-desktop --request-timeout=10s wait --for=condition=Ready nodes --all --timeout=180s
kubectl --context docker-desktop --request-timeout=10s get pods --namespace kube-system
kubectl --context docker-desktop --request-timeout=10s get nodes \
  -o custom-columns=NAME:.metadata.name,CPU:.status.allocatable.cpu,MEMORY:.status.allocatable.memory,PODS:.status.allocatable.pods
```

Traiter un dépassement de délai comme un besoin de diagnostic, pas comme une vérification réussie. Distinguer les pods système indisponibles des jobs terminés. La [commande wait](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_wait/) teste une condition ; les [conditions du nœud](https://kubernetes.io/docs/concepts/architecture/nodes/) décrivent son état déclaré.

Prévoir les ressources pour Kubernetes, services de contrôle ai-guard, workers, constructions d'images et éventuel modèle local exécutés ensemble. Dans Docker Desktop, examiner les allocations CPU, mémoire et disque sous **Settings → Resources**. Conserver une marge pour le poste et les données persistantes. Voir les [réglages de ressources Docker](https://docs.docker.com/desktop/settings-and-maintenance/settings/#resources).

La dernière commande indique la capacité allouable, pas la consommation actuelle. Dimensionner les requests et limits des workloads selon cette capacité, puis mesurer l'usage réel pendant le parcours prévu ; voir la [gestion des ressources Kubernetes](https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/). Un nœud Ready ne démontre ni haute disponibilité ni capacité de 100 workers.

## Préparer les images

Construire les images ai-guard selon le [guide local](../local/README.fr.md) et, pour Codex ou Claude Code, le [guide fournisseurs](../providers/README.fr.md). Conserver tags sélectionnés, identités d'images et architecture. Aucune image publique ai-guard préconstruite n'est exigée.

Vérifier le magasin d'images pris en charge par le provisionneur avant de préparer leur livraison aux pods. Le provisionneur `kind` de Docker Desktop exige son magasin containerd ; changer de magasin masque les images et conteneurs du magasin inactif tout en les conservant sur disque. Voir le [guide du magasin d'images Docker](https://docs.docker.com/desktop/features/containerd/) et la [compatibilité des provisionneurs](https://docs.docker.com/desktop/use-desktop/kubernetes/#cluster-provisioning-method).

Une construction Docker seule n'établit pas qu'un nœud Kubernetes peut résoudre l'image. Préparer les références exactes et un chemin de livraison approuvé pour le runtime du nœud choisi. La disponibilité de l'image et `imagePullPolicy` déterminent ensemble le démarrage d'un pod ; voir les [images locales et téléchargées](https://kubernetes.io/docs/concepts/containers/images/). Garder les credentials de registre dans le mécanisme approuvé par l'opérateur.

## Diagnostiquer un échec de préparation

| Symptôme | Action |
|---|---|
| Contexte `docker-desktop` absent | Vérifier la progression de création dans Docker Desktop et l'installation/configuration kubectl prévue. Conserver les autres contextes. |
| Erreur TLS ou de connexion | Vérifier serveur sélectionné, état de Desktop, confiance du certificat et horloge du poste. Garder la vérification active. |
| Nœud absent ou non Ready | Vérifier création dans Desktop, mémoire/disque disponibles et accès aux images nécessaires ; reprendre la vérification bornée après correction. |
| Pod système indisponible | Examiner l'état du pod nommé dans la vue Kubernetes de Docker Desktop et obtenir un diagnostic ciblé. Garder les logs bruts privés et masquer tout extrait partagé. |
| Échec de téléchargement d'image | Vérifier référence exacte, architecture, magasin sélectionné et accès approuvé au registre. Conserver images et volumes existants. |

Relever versions Desktop/client/serveur, provisionneur, nombre de nœuds, contexte sélectionné et erreur précise. Partager uniquement des métadonnées relues. Exclure des rapports kubeconfigs, tokens, URL d'authentification et environnements complets.

## Utiliser et maintenir l'environnement

Conserver données de contrôle et credentials ai-guard séparés de l'administration Kubernetes. Suivre la [procédure Docker/Podman](../local/README.fr.md) pour les sessions d'agents locales, et la [configuration entreprise](../enterprise/README.fr.md) pour les exigences d'identité, stockage et transport d'une installation administrée.

Évaluer la disponibilité de la plateforme selon le workload prévu : ressources disponibles, accès aux images, conservation du stockage, restrictions réseau appliquées et reprise après interruption. Vérifier les effets des politiques réseau avec l'implémentation réseau choisie ; une liste de NetworkPolicy ne démontre pas l'isolation. [Exigences des politiques réseau Kubernetes](https://kubernetes.io/docs/concepts/services-networking/network-policies/).

Avant une maintenance planifiée de Desktop, arrêter les workers ai-guard actifs avec leur broker et préserver les données de contrôle. Après réouverture de Desktop, reprendre les vérifications TLS et de disponibilité sur le même contexte. Conserver cluster et volumes existants pendant le diagnostic ; planifier tout remplacement ou mise à jour du cluster séparément, avec une procédure explicite de conservation des données.
