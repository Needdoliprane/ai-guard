# Inférence locale réutilisable — essai ciblé du 1er novembre

**PASS pour cet échantillon HTTP et mémoire ; FAIL historique du jugement de sécurité conservé.** Luna a exécuté quatre cas de transport puis une seule inférence synthétique, sans projet ni credential. Aucun appel fournisseur payant. Le backend réutilise les poids Qwen déjà essayés pour Jev ; ce lot ne livre pas encore l'adaptateur produit ou les parcours Codex/Claude d'entreprise.

| Vérification | Résultat constaté |
|---|---|
| Ancien probe, réponse fragmentée | RED attendu : RC2, `chunk decode: bad chunk line`. Le serveur envoie les en-têtes avant les chunks, séparés par des attentes. |
| Probe corrigé, même corps fragmenté | PASS : RC0, HTTP200 et corps exact `{"ok":true}`. |
| Corps tronqué | PASS du refus : RC2, `invalid/oversized chunk body`. |
| Serveur silencieux après les en-têtes | PASS du refus : RC2 sur lecture bornée ; aucune réponse présentée comme valide. |
| Inférence réelle unique | PASS : HTTP200, modèle retourné `qwen2.5-0.5b-q4km`, réponse non vide de 8 scalaires Unicode. [Requête](smoke/request-smoke.json) et [sortie réelle](smoke/post.stdout). |
| Mémoire observée sur ce serveur | Pic cgroup 788877312 octets, plafond effectif 999997440 ; compteurs OOM/OOM-kill/max nuls, swap courant et plafond nuls. [Lecture](smoke/cgroup.stdout) et [commande avec ordre des champs](smoke/cgroup-command.txt). Ce n'est pas la cible 500 Mo. |
| Arrêt du serveur créé pour cet essai | Commande d'arrêt RC0 ; inspection du même CID `exited 0`. Délai de grâce Docker 5 s, borne caller 15 s puis kill 1 s. Objets conservés, aucune suppression. |
| Jugement de sécurité | **FAIL conservé** : le [cas du 30 octobre](../../2026-10-30/local-judge/README.md) autorisait une suppression. Une salutation réussie ne le corrige pas. |

Les compteurs 40/3/43 et cached 0 de la réponse sont **déclarés par le serveur**, pas une mesure indépendante de facturation. La revue les a vérifiés comme entiers non négatifs et 40+3=43. CPU 2 est une allocation ; aucune consommation CPU ou distribution de latence n'est qualifiée.

## Sources et reproduction

Docker client 29.2.1, moteur 29.9.2 Linux ARM64 sur Docker Desktop/macOS ARM64. Poids `Qwen2.5-0.5B-Instruct Q4_K_M`, 491400032 octets, fichier 0444, SHA256 `74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db`, révision officielle `9217f5db79a29953eb74d5343926648285ec7e67`. Image llama.cpp `server-b11718@sha256:b0a70ac8e2d1e663ba36a98e3706301e4a4a9b6094c9f99398f0928a8dd2ea62`, ID local `c7d4e00957124c84ed75760902ee8e20597197924af37f0c4712463baa59f232`. Les poids et images restent hors Git. La [provenance d'acquisition](acquisition-provenance.md) décrit l'étape antérieure aux exécutions.

Les instruments Rust sont hors application : [probe corrigé](probe/probe.rs), [ancien probe](fixture/probe-legacy.rs) et [serveur synthétique à quatre connexions](fixture/fixture-v2.rs). La seule correction du probe est la lecture du corps fragmenté avant son décodage ; aucun client HTTP général ou streaming produit n'est livré. Requête 16 KiB/réponse 64 KiB maximum, connexion 3 s, écriture/lecture 5 s et budget réseau coopératif 180 s, démarré **après lecture de stdin**. La commande POST possède une borne externe distincte de 190 s puis kill 1 s ; son entrée est le fichier synthétique fini. Le serveur de fixture borne aussi acceptation et lecture des requêtes. Les Dockerfiles épinglent Rust 1.99.1 ; aucune dépendance Cargo n'est introduite.

Les [résultats parser](parser/final-inspects.txt) distinguent le premier incident de sélection d'image : `/probe-legacy` absent, état `created`, RC127, aucune connexion à la fixture. Ce n'est pas le RED. Après correction du choix d'image, les quatre clients ont les codes 2/0/2/2 et le serveur de fixture termine avec le code 0. Les captures `.stdout`, `.stderr`, `.rc` restent séparées, sans remplacement d'un échec par un succès.

Les [commandes serveur](smoke/server-command.txt), [POST](smoke/post-command.txt), [arrêt](smoke/server-stop-command.txt) et [inspection finale](smoke/server-poststop-inspect.txt) sont celles de l'essai. Leurs chemins, nom de réseau, CID et IP sont historiques ; ne pas les réutiliser sans préparer des ressources neuves et vérifier les identités. Les commandes complètes des quatre probes parser n'ont pas été archivées dans ce dossier : sources, sorties et inspections sont conservées ; aucune commande reconstruite n'est présentée comme une capture exécutée.

Serveur sans port publié sur un réseau bridge `internal=true`, rootfs en lecture seule, UID 65532, capabilities supprimées, no-new-privileges, CPU 2, PID 64, mémoire/swap demandés 1000000000. Seul le poids est monté en lecture seule. Clients sans montage ou port publié, UID 1000, rootfs en lecture seule et bornes 64 Mo/PID 32. Ces réglages inspectés ne remplacent pas un test actif d'egress ou d'évasion. Le serveur de modèle est **arrêté** à la fin ; il n'est pas annoncé comme service produit permanent.

## Relecture et limites

Le validateur Rust privé a accepté la réponse enregistrée, mais sa revue a constaté deux tolérances JSON (espaces VT/FF et clés dupliquées) et un contrôle incomplet des types de compteurs. Il ne constitue pas un validateur JSON strict général. Le reviewer a reparsé **le même échantillon** indépendamment : JSON standard complet sans clé dupliquée, modèle exact, contenu non vide, compteurs valides. Aucune nouvelle inférence n'a été lancée pour cette vérification. L'incident de permission 126 du premier lancement du validateur a été rapporté par l'exécutant ; le fichier final `.rc` ne conserve que le 0 ultérieur. Ne pas prétendre disposer ici d'un journal durable du 126.

**NOT_RUN** : adaptateur local Jev produit, configuration dynamique, Codex Responses/CLI et Claude Messages/CLI, authentification entreprise, TLS/chaîne d'AC, concurrence, capacité, Podman, autres plateformes et contrôle actif du réseau. **FAIL** : qualité du modèle comme autorité d'autorisation. Aucun gate production fermé, aucune facturation activée. La prochaine verticale doit fournir le raccord Rust/configuration et les parcours clients avec leurs refus, sans repli cloud.

Sources primaires : [poids Qwen épinglés](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/tree/9217f5db79a29953eb74d5343926648285ec7e67), [llama.cpp](https://github.com/ggml-org/llama.cpp), [Dockerfile Rust épinglé](https://github.com/rust-lang/docker-rust/blob/de3b3171ec576fd91eb8e28d3d37fa117b135fcb/stable/bookworm/slim/Dockerfile). [ARTIFACTS.sha256](ARTIFACTS.sha256) identifie les fichiers publics ; les captures originales privées restent conservées.
