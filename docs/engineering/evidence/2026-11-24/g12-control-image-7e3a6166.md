# Image de contrôle locale du candidat 7e3a6166

Le 24 novembre 2026, Docker Desktop ARM64 a construit
`images/control.Dockerfile` à partir des seuls chemins requis par la
recette, extraits avec `git archive` depuis
`7e3a6166df4b150aadffb82cae1b46f20e4aaa9e`.
Le contexte avait pour SHA-256
`7577a9e2cc8660312c157d690f2153197cc24e39661c0ee11ca4ced85a4110c3`.
Le CLI embarqué a annoncé la révision complète et la version 0.0.1.
L’image Linux ARM64 utilise l’UID et le GID 1000. Son identifiant
de configuration est
`sha256:29bb4a5f0ec5b1fbaf9657979a7d491d23be473aea59d320a6dd2573b23192ec`.

**PASS ciblé** sur Docker Compose 5.7.1 et Podman rootless avec Docker
Compose 5.0.2. Le transfert `docker image save | podman load` a conservé
l’identifiant de l’image. Les deux piles ont atteint l’état sain sur les
trois services. Seuls les sous-réseaux de la recette Docker ont été
surchargés. Les fichiers inventoriés dans l’image et servis par chaque
ingress avaient les mêmes SHA-256.

| Fichier | SHA-256 |
| --- | --- |
| `index.html` | `c23307676bb73cf4c9efb28517c37e5d9d9e30cccb208c1f2f2bd2b73a84d955` |
| `assets/index-u7jpYl5g.js` | `7fdcaefb0816e546045bd2caee8b818e5af9620fbd138368641fbd40d8dad2c6` |
| `assets/index-Bx1Iwo7b.css` | `bad4d35f5a132e85484697387b31d06b532b0c1dc7c258e53ec26cbcd62d481b` |
| `favicon.svg` | `51a233c71906900b8589ebef7537429dbb42cc4bcb631f942658dc8601893853` |

Les conteneurs, réseaux et volumes des deux projets de recette ont été
retirés. Les images sont conservées localement.

**FAIL construction native Podman rootless.** Le même contexte et la
même révision ont échoué sur la VM locale à 3814 puis à 7168 Mio.
Dans chaque cas, le journal noyau identifie un arrêt de `rustc` par
manque de mémoire pendant la compilation Rust release. Le second arrêt
est intervenu à environ 6 Gio de RSS anonyme pour le processus. La VM
a retrouvé son allocation initiale et son API rootless était joignable.

Ces reçus couvrent la construction Docker et le démarrage de cette image
sur deux moteurs ARM64. La construction native Podman et la publication
GitHub restent ouvertes.

## Rejeu après les corrections de dépendances

**PASS ciblé** sur `fdf780c3419db12d6f2e9d63d54004c5cacc4108`,
qui inclut les mises à jour de `url` et `jsonwebtoken`. Docker ARM64 a
reconstruit la recette depuis le contexte Git archivé. Le CLI embarqué
annonce cette révision. L’image Linux ARM64, UID/GID 1000, a pour
identifiant
`sha256:3e11af80804846aed58dda3b209ed86ddf9d9c1807e250991882b75d1127640e`.

Après transfert, Podman rootless conserve cet identifiant. Les trois
services Compose sont sains sur chaque moteur. Les quatre fichiers
frontend servis correspondent aux fichiers embarqués. Les conteneurs,
réseaux, volumes et credentials des deux recettes ont été retirés.
Les 65 paquets source Debian ont les mêmes versions sur les deux
moteurs et dans le relevé antérieur de `7e3a6166`. Le verdict de
construction native Podman et le statut de publication restent ouverts.
