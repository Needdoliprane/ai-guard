# Autorité humaine — primitives de propriété et fichiers

Le 11 novembre 2026 à 21:47 UTC, le gel Linux de ces primitives obtient **33 parents automatiques PASS** (25 owner/epochs et 8 filesystem), **un négatif Docker exécuté explicitement PASS**, le build lib et `cargo check --all-targets` PASS. Chaque parent sélectionné compte un succès, aucun échec ni ignoré. La liste réelle contient 971 tests ; elle ne signifie pas 971 tests exécutés.

## Ce que couvre ce lot

- Propriété OS retenue : exclusion par flocks réels, ordre/déduplication des parents physiques, garde conservée par les tokens et les tâches acceptées, y compris après annulation de l'observateur.
- Manifest et revalidation : fermeture/configuration exactes, refus des alias/liens ou remplacements de source/lock, contrôles de mode et diagnostics sans chemin/contenu privé.
- Epochs lecture/écriture : ordre, attente bornée, drainage à la fermeture et invalidation typed. Les scénarios EMFILE/ENFILE contrôlent la classification structurée et la conservation de l'owner ; **aucun épuisement réel de descripteurs n'a été lancé**.
- Observation Linux du domaine de montage : le négatif séparé vérifie les métadonnées d'un fichier déjà monté dans le conteneur de QA (domaines 1138 et 1181 distincts), sans lire son contenu ni créer de montage. Le test normalement ignoré est réellement sélectionné avec `--ignored` ; il n'est pas compté comme un ignoré réussi.

Les quatre sources nouvelles et les deux déclarations de modules sont tracées avant/après dans [SOURCE_MANIFEST.json](SOURCE_MANIFEST.json). Les postimages correspondent aux versions relues owner r3 / filesystem r2 ; les deux seuls raccords sont `pub mod human_authority;` et `mod source_owner;`. Aucune route, migration, ouverture moderne ou activation Ready n'est ajoutée par ces raccords.

## Source et commandes observées

Base publique `4b1927f03ab9be76aefbfbec56dfc21a7ad52abf` : 2 500 fichiers, quatre ajouts et deux déclarations de modules modifiées. Source d'entrée : `a9fcb80227dfae5ef1275d3c89ccf322b9c470761a161ab7b414aa15aeaca9bd`. L'archive réellement exportée avant arrêt contient 2 504 payloads réguliers0644, tous identiques au manifeste d'entrée : `1f15580d61636a9de8bfae9781d83580da87b52dbdfb7d537abb4ab55e308cab`, 27 729 920 octets. Les empreintes des 34 noms/cibles/RC/sorties et des commandes sont dans [TESTS.json](TESTS.json). Aucun journal brut ou chemin privé n'est publié.

Ordre observé : `cargo test --frozen --offline --lib --no-run --message-format=json`, liste lib, 33 parents automatiques exacts, `cargo check --frozen --offline --all-targets --message-format=json`, puis le négatif Docker optionnel exécuté explicitement. RC 0 pour ces phases, avec Cargo JSON terminé en succès pour le build et le check. L'artefact lib résolu sert à chaque test exact ; le helper de sous-processus n'est pas un parent indépendant. L'intégrateur a contrôlé 631 artefacts de QA.

QA Linux aarch64, kernel observé 7.0.14-linuxkit, toolchain épinglée 1.99.1, image épinglée, UID1000, réseau absent, racine RO, capabilities retirées, no-new-privileges/seccomp. Limites : 4 CPU, 9 Gio sans swap, 128 PID, scratch tmpfs4Gio, Cargo hors ligne à un job. Fermeture ciblée RC 0, aucun OOM observé ; mémoire finale courante 1 451 266 048 octets et pic 6 229 848 064 octets. Ces mesures décrivent le cgroup de QA, pas un worker, une facturation ou une performance produit.

## Échec historique conservé

La première campagne r1 a échoué en préparation (RC 1) sur deux chemins de pins de cache obsolètes : **zéro compilation et zéro test**, avant extraction. Son export réellement vide de 10 240 octets (SHA `cbe4dd8a49c2125ca7ff2f8e437f147d7c15f306b8faa49dc2e6af6fda8faceb`) n'est pas la source de 2 504 fichiers. Le correctif r2 porte uniquement sur les chemins du harnais, sans changer source, profil ou oracles. Cet échec ne devient pas un RED produit ni un PASS transféré.

## Limites

**Non qualifiés par ce lot :** approbation nominative complète/ApproveLaunch, Store14, AuthLedger5, migrations et startup Ready, SSO et tous mutateurs, maintenance r4 et index/Store r2, runtime Docker worker, Podman, Kubernetes, macOS natif, épuisement réel de FD, charge100, Clippy strict et suite complète. La présence du module ne confère aucune permission humaine. Les documents de preuve ajoutés après la campagne et le checkout vivant ne sont pas rétroactivement couverts. Aucun gate ou fingerprint global fermé ; E reste exclusivement chez Claude.
