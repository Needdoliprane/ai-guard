# Invocation — recherche bornes et ACL

29 octobre2026. Recherche de conception/lecture seule, hors audit formel. Aucun code du dépôt modifié, aucune cible, moteur ou helper exécuté dans ce sous-lot. Sources du dépôt observées avant le futur ancrage FD. Les constats ne qualifient aucune plateforme.

## Lecture bornée du binaire

`identify` lit actuellement jusqu'à EOF (`src/runtime/invocation.rs:179`). Le buffer64KiB borne la mémoire, pas le total : un fichier très grand ou continuellement agrandi peut monopoliser la vérification avant le timeout CLI.

Politique proposée : plafond constant512MiB, suffisant comme choix d'ingénierie initial à documenter, ou1GiB si un artefact réel l'exige. Refuser la taille initiale supérieure au plafond avant de lire. Sur le descripteur régulier déjà ouvert sans suivre de lien et en nonblocking, streamer au plus `initial_len + 1` octets par blocs64KiB ; exiger un total égal à `initial_len`, puis fstat final avec identité, taille, mode, uid/gid, mtime et ctime inchangés. Un EOF prématuré et un octet supplémentaire sont des refus. `Read::take` permet cette borne ; une simple comparaison metadata après une boucle jusqu'à EOF ne suffit pas. [Documentation Rust Read/take](https://doc.rust-lang.org/std/io/trait.Read.html#method.take).

La borne garantie est alors512MiB+1octet lu, avec mémoire constante. Elle ne borne pas le temps mural d'un read régulier bloqué par NFS/FUSE ou une panne disque : `O_NONBLOCK` ne transforme pas les I/O de fichiers ordinaires en opérations interruptibles. Un délai Tokio autour d'une lecture synchrone ne constitue pas une telle preuve. Conserver cette limite explicite ; aucun timeout fort de lecture n'a été démontré ici.

Tests pertinents : fichier sparse immédiatement supérieur au plafond (refus sans hash intégral), fichier qui grandit sans EOF, troncature, taille exacte autorisée et préservation de l'identité. La fixture FIFO existante couvre un autre problème : blocage à open avant fstat.

## ACL du fichier et du répertoire final

**macOS : mode022=0 seul ne suffit pas.** Les ACE allow/deny sont évaluées avant le recours aux permissions BSD et peuvent accorder écriture, ajout/suppression ou changement des règles. Un fichier ou répertoire755 peut donc avoir une autorité d'écriture supplémentaire. Les permissions sensibles comprennent write_data/add_file, append_data/add_subdirectory, delete/delete_child, write_attributes, write_extattributes, write_security et change_owner. L'ordre, les UUID/groupes imbriqués et inheritance rendent une évaluation partielle fragile. [Modèle Apple](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemDetails/FileSystemDetails.html).

Approche minimale vérifiable : pour les deux leaf FDs, refuser toute ACL étendue non vide. Utiliser `acl_get_fd_np(fd, ACL_TYPE_EXTENDED=0x100)`, puis libérer via `acl_free`. Les déclarations existent dans le SDK local sys/acl.h, mais pas dans la surface libc Rust actuelle ; un petit FFI Darwin documenté est nécessaire. Une API par chemin réintroduirait une résolution concurrente. Les erreurs de lecture, EACCES et ENOTSUP ne sont pas une preuve d'absence. [Man Apple acl_get_fd](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/acl_get_fd.3.html).

Deux subtilités Darwin à tester dans la qualification :

- `acl_get_fd_np` peut retourner NULL/ENOENT quand FILESEC_ACL est absent ; ce cas diffère d'une erreur générique. L'implémentation officielle passe par fstatx_np puis filesec_get_property. [Apple acl_file.c](https://github.com/apple-oss-distributions/Libc/blob/main/posix1e/acl_file.c#L66), [Apple filesec.c](https://github.com/apple-oss-distributions/Libc/blob/main/gen/filesec.c#L141).
- `acl_get_entry(FIRST)` retourne **0 pour une entrée obtenue**, -1/EINVAL pour une ACL vide/fin, contrairement au schéma Linux1/0. Un acl_t retourné valide et FIRST suffisent pour la politique «aucune entrée». [Apple acl_entry.c](https://github.com/apple-oss-distributions/Libc/blob/main/posix1e/acl_entry.c#L109).

**Linux POSIX ACL : la portée des modes est plus forte.** Le groupe du mode reflète ACL_MASK s'il existe ; ce mask borne les droits effectifs de tous les named users/groups et du groupe propriétaire. Ainsi owner root/contrôle + mode022=0 exclut l'écriture via une POSIX access ACL par les autres comptes. Une default ACL du répertoire concerne l'héritage des nouveaux enfants, pas une autorisation actuelle d'écrire le répertoire. [acl(5), correspondance/mask](https://man7.org/linux/man-pages/man5/acl.5.html).

Cette conclusion vise les POSIX ACL du filesystem local qualifié ; elle ne prouve pas les ACL NFSv4/SMB, un filesystem FUSE arbitraire ou les privilèges root/capabilities. Si une politique conservatrice veut aussi refuser les POSIX ACL présentes, `fgetxattr(fd,"system.posix_acl_access",NULL,0)` évite une allocation ; taille positive => présent/refus. Mais ENODATA peut aussi indiquer absence d'accès, et ENOTSUP signifie attributs non supportés/désactivés : ne pas en déduire «aucune autre ACL sur tout filesystem». [getxattr(2)](https://man7.org/linux/man-pages/man2/getxattr.2.html). Le contrôle mode022 reste la preuve POSIX précise ; un inventaire global des mécanismes d'ACL exige qualification du type de filesystem.

## Helpers : résidu séparé du FD moteur

Le PATH actuellement fixé inclut `/opt/homebrew/bin`, constaté ici louis:admin775. Docker recherche `docker-credential-<credsStore/credHelpers>` dans PATH ; la configuration standard du compte peut donc déclencher un programme secondaire de ce répertoire. Pinner le seul binaire principal n'élimine pas cette influence admin. [Docker CLI configuration](https://docs.docker.com/reference/cli/docker/), [Docker login/helpers](https://docs.docker.com/reference/cli/docker/login/).

Sous une politique qui n'accorde plus automatiquement l'autorité aux membres admin, ce segment doit être refusé/omis, ou ces helpers doivent eux aussi recevoir une liaison approuvée. Une validation de répertoire par pathname avant usage ne le pinne pas contre le renommage d'un ancêtre modifiable ; les propriétés du FD principal ne se propagent pas aux exec transitifs. Limiter PATH aux répertoires système strictement protégés est simple, mais peut casser les helpers Docker Desktop/Homebrew : cette compatibilité doit être traitée et prouvée, pas présumée. Même observation pour les plugins CLI sélectionnés par la configuration du compte : leur confiance doit être explicitement comprise dans la frontière retenue.

Conclusion de ce sous-lot : bornage en octets recommandé ; API ACL fd Darwin identifiée ; garantie POSIX Linux précisée ; durcissement du CLI principal seul insuffisant pour annoncer la suppression de toute délégation admin sur les helpers. Revue finale après gel du code FD encore à faire.
