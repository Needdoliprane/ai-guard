# Qwen V3 — avis indépendant limité aux preuves capturées

1 novembre 2026. Revue en lecture seule des captures images-v3.hWSSkr, parser-dd46e6c0-350d-47df-ac12-b4c444a78bc0, sources parser-review-20261101 et smoke-b610e327-2871-4381-a550-459e573ad2c6, sous /private/tmp/. Aucun Docker/Cargo, test, modèle ou nouvelle requête exécuté par le reviewer. Pas de credential, Env/Cmd moteur ou autre fixture privée consulté. Analyse/hachage de fichiers autorisés seulement. Worktree produit broker-service constaté propre à2bcdb629abe879b8ceaa79ad81cb720cdc7e87ba.

## Provenance et intégrité

Les69 entrées du manifeste images et les16 du manifeste smoke concordent toutes. SHA manifestes : images456aa532d737b5908e5d434189f63bde1059b7283d7cc95fdd44308902f15a11 ; smoke f94cede5d565cc6966724a57f1767c3c714dad58ebaf2fa3e53a76d1e9467614. Ces manifestes couvrent leurs sélections, pas tout l'historique de commandes. Aucune archive portable supplémentaire fournie dans ce lot ; ne pas extrapoler une garantie source-only/absence d'AppleDouble à une future archive assainie.

Acquisitions explicites RC0, base Rust index24707e70…/enfant ARM64 efecb769… et llama registry b0a70ac8… reliés aux image IDs observés. Build probe --pull=false/--network=none ; run par image ID avec --pull=never, aucune substitution de tag. Image modèle c7d4e009… et probe92861eaa… reliées aux observations du run. Source V3 et copie image-context identiques40d78b0047642c8f63328c0ea752d8922cad411edb61c0c711a8760d16e62178 ; Dockerfile efc353fb57bc29823762b65bc5f652ec47dd35cf1f9dcbf3d31c0637d9e1e2d7. La preuve d'acquisition n'est pas une signature de chaîne de fourniture ni une qualification release.

Préflight consigné : poids491400032 octets, mode0444, SHA74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db ; révision Qwen9217f5db… dans la provenance historique. Le reviewer ne relit pas les491Mo. Les métadonnées/hash pré-start et le montage observé RO établissent cette preuve dans la frontière de l'hôte de contrôle ; pas un scellement contre root/UID propriétaire.

## Parseur : résultats causaux mais bornés

Fixture finale b1f6e46f7599270219f41ce6b327c26f120bab102306ab53e9d0015b291d5573 : quatre acceptations, réponse chunked correctement formée en écritures séparées pour legacy/V3, fermeture tronquée, puis silence. Requête entièrement lue sous borne/deadline ; erreurs d'écriture fatales pour V3, tolérance BrokenPipe/Reset uniquement pour la fermeture legacy attendue. Build observé utilise fixture-v2, pas fixture initiale invalide conservée sans exécution.

Legacy1127cd080a5de5ccb62864f2ce86f8fc4b7377abd37782cf129ccd7086a1d773 : RC2/bad chunk line ; V3 : RC0, statut200 et corps exact{"ok":true} ; tronqué : RC2/invalid-or-oversized chunk body ; silence : RC2/body-read OS11 sous read_timeout5s, fixture silencieuse7s. La durée par cas n'a pas été chronométrée indépendamment par cette revue. Fixture finale exited0. Le premier mauvais choix d'image/entrypoint a RC127, état created/non-running : ce n'est ni un RED du parseur ni un programme terminé après exécution. Quatre appels parser exécutés, plus cet échec de lancement distinct.

Le reader reste minimal Connection:close, avec corps/framing bornés ; ambiguïtés d'en-têtes, trailers et client HTTP général non qualifiés. Les180s du code commencent après lecture stdin : ne pas les présenter comme borne dure de tout le processus. Le caller POST190s+kill1s constitue la borne externe distincte ; l'entrée de ce run est un fichier synthétique fini.

## Unique inférence et validation de l'échantillon

Une seule health200 et une seule POST Chat Completions ; requête JSON connue max_tokens64/température0/stream=false, aucune réinférence. POST RC0. SHA de post.stdout complet (ligne statut comprise),639octets : b4c87043f33912772fb5708f98b58b066b0ea5c192db38b37cc9f62c14b5da23. Le reviewer a parsé indépendamment le corps existant avec un parseur JSON standard et refus des clés dupliquées : JSON complet valide, modèle retourné qwen2.5-0.5b-q4km, contenu non vide de8scalars Unicode, compteurs entiers non négatifs déclarés40+3=43 et cached0. Aucun texte modèle exécuté ni permission issue de sa réponse.

Défaut de portée du validateur privé476130b59f30a57c1c9f75c68940fcaa4e33dca6a852b5f4a3c7ffa56ec5bbad : Parser::ws accepte VT/FF via is_ascii_whitespace ; Object::insert écrase les clés dupliquées ; usage seulement exigé objet, pas compteurs typés/cohérents. Un VT avant une enveloppe sinon valide serait accepté à tort comme JSON. fs::read précède la vérification64KiB. C'est un parseur structuré, pas un simple contains, mais aucun validateur JSON général strict/borné n'est qualifié. Ces faiblesses n'invalident pas l'échantillon vérifié indépendamment. Root conserve cette limite, sans correction générale ni nouvel appel modèle pour ce lot.

Incident126/permission600 du premier lancement du validateur rapporté par Root ; aucune trace durable distincte retrouvée dans les RC/stdout/stderr/notes/commandes des trois captures autorisées. Ne pas fabriquer de pseudo-log rétrospectif. Le chmod700, le RC0 et la validation du même post.stdout sont durables. Les répétitions des trois tests unitaires du validateur ne constituent ni nouvelles fonctions uniques ni nouvelles inférences.

## Ressources et arrêt observés

Même CID serveur e875a4dd… recoupé entre cidfile, commande, scalaires pré-stop et inspect final. Image épinglée, UID65532, rootfs RO, ALL caps dropped/no-new-privileges, pids64, allocation2CPU,1e9 mémoire/swap demandés ; seul bind modèle RO, aucun port publié, réseau IDaeed465b…/bridge/Internal=true. Pas de preuve d'egress comportemental ou d'isolation tenant par cette observation. server-inspect.json est du texte formaté, pas JSON : conserver l'original privé et nommer correctement la copie publiée.

cgroup RC0 : peak788877312 < max999997440, événements OOM/kill et autres compteurs0, swap.current0/swap.max0. Le pic couvre la vie du serveur jusqu'au relevé (chargement/health/inférence), pas un delta de mémoire imputé au seul appel, ni la mémoire totale du poste/VM. CPU2 est une allocation, pas une consommation mesurée. Usage token déclaré ne constitue ni mesure indépendante de tokenizer ni facturation.

Les deux probes sont observés exited0. Docker stop exact-CID RC0 avec grace5s et caller15s+kill1s, puis même CID serveur exited0 ; pas de déduction d'arrêt à partir du seul timeout ou du seul retour Docker. Objets conservés, aucun rm/prune. Les six objets de build précédents sont distincts ; created n'est pas un terminal exécuté.

Avis : aucun défaut bloquant étayé pour publier cette preuve étroite d'acquisition, protocole et ressources d'un unique échantillon, avec les limites ci-dessus et l'incident126 seulement rapporté. Le FAIL sémantique historique du juge reste FAIL. Aucune équivalence Jev, qualité d'autorisation, Responses/tools/Claude/Codex, inférence entreprise/TLS/G11, performance générale, CPU consommé ou facturation n'est démontrée.
