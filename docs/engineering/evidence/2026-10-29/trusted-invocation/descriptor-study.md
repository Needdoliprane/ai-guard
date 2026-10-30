# Décision proposée : invocation ancrée au répertoire final

29 octobre 2026 — étude seule, aucune modification du dépôt ou opération moteur. Hôte observé : Darwin27.2.0 ARM64 ; Rust1.99.1 ; SDK /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk.

## Décision concrète

**Recommandation : adopter un descripteur du répertoire final et une invocation `./basename` après `fchdir` dans `pre_exec`, sur macOS et Linux.** Exiger leaf directory et fichier exécuté appartenant à root ou au contrôle, sans permissions022, et contrôler leurs ACL. Conserver descripteur privé vivant à travers clones Runtime/Command, identités durables du répertoire et du fichier, plus SHA256 borné. Le lookup d'exécution ne repasse plus par les ancêtres group-write. Ne pas accepter gid80 automatiquement comme barrière de sûreté.

**Cela exige une décision explicite sur le cwd : le processus moteur s'exécutera dans le répertoire final ancré de son CLI, hors projet ; son cwd ne sera plus '/'.** La politique d'invocation et ses données doivent le déclarer. Le test natif le prouve. On ne peut pas faire `fchdir(fd)`, remettre cwd='/' puis exécuter `./basename` : le dernier lookup résout à partir de '/'. La seule séquence portable réellement observée conserve le cwd leaf. Aucun texte ne doit encore promettre cwd='/'. Les source/build/Compose/env-file restent des chemins absolus fournis par le caller.

Si le cwd='/' est impératif, ne pas livrer une émulation `/dev/fd` supposée fonctionnelle sur macOS : les deux variantes testées échouent EACCES. Linux fournit execveat/fexecve, mais leur disponibilité n'est pas une preuve de mécanisme équivalent Darwin. Cela nécessiterait une autre conception à qualifier, pas un ajustement silencieux.

## Preuve minuscule indépendante exécutée

Artefacts conservés :

- /tmp/ai-guard-descriptor-probe.kMqelu/probe.rs : driver Rust natif sans dépendance et deux payloads Rust non moteur ; compile avec le rustc fixé puis teste les invocations.
- /tmp/ai-guard-descriptor-probe.kMqelu/result.txt : sortie brute ci-dessous.

Le driver crée un leaf privé contenant APPROVED et ouvre son FD. Il renomme ce leaf en parked puis crée au chemin initial un autre leaf contenant SUBSTITUTED. Il n'altère aucun binaire installé ni aucun fichier du dépôt. Les children sont des exécutables natifs Rust sans réseau, credential ni effet hors de ce répertoire temporaire.

```text
pathname_after_parent_swap: status=exit status: 0 stdout="SUBSTITUTED cwd=/\n" stderr=""
directory_fd_then_relative_exec: status=exit status: 0 stdout="APPROVED cwd=/private/tmp/ai-guard-descriptor-probe.kMqelu/parked\n" stderr=""
dev_fd_cloexec: spawn_error=Os { code: 13, kind: PermissionDenied, message: "Permission denied" }
dev_fd_no_cloexec: spawn_error=Os { code: 13, kind: PermissionDenied, message: "Permission denied" }
```

Séquence réellement exécutée pour la variante ancrée : `File::open(leaf)` reste vivant ; `Command::new("./runtime").current_dir("/")` ; `pre_exec` ne fait que `fchdir(dirfd)` et retourne une erreur système si échec ; puis spawn natif. CLOEXEC ferme le FD à exec après son utilisation. La variante /dev/fd a été testée avec le flag conservé et avec sa suppression via fcntl dans pre_exec : même EACCES.

Le résultat montre la résistance au remplacement d'un ancêtre/nom après ouverture du répertoire. Il ne prouve pas une résistance à root, au propriétaire du leaf/fichier ou à une écriture permise par ACL. Il ne prouve pas Linux ni Docker/Compose : ces validations restent à exécuter après implémentation.

## Implémentation recommandée, après autorisation

1. À l'approbation, résoudre l'entrée statique/explicite ; exclure projet et cible hors racines admises pour la fabrique ; ouvrir le répertoire final via `O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC`. Vérifier fstat directory, owner root/contrôle, mode022=0 et ACL. Conserver le FD dans Runtime, pas seulement le nom du répertoire. Son nom canonique est une provenance ; son dev/inode est l'identité durable à vérifier lors d'une réouverture.
2. Ouvrir le basename relativement au dirfd (`openat`, O_NOFOLLOW|O_CLOEXEC|O_NONBLOCK), fstat fichier régulier exécutable, owner root/contrôle, mode022=0 et ACL. Calculer le hash borné sur ce FD et comparer identité/métadonnées. Lors d'une réouverture persistée, acquérir de nouveaux FDs et exiger les mêmes identités répertoire/fichier et SHA256 ; aucune recherche PATH ou adoption automatique.
3. Avant chaque effet, revérifier le répertoire détenu et ouvrir/vérifier le basename par openat sur ce FD. Fabriquer `Command("./basename")` ; env_clear et environnement approuvé ; pre_exec fchdir du dirfd. Le Command doit retenir le FD jusqu'au spawn, même si le Runtime appelant a été cloné ou détruit. pre_exec ne doit contenir ni allocation, ni verrou, ni log, ni hash/ACL ; toutes ces vérifications sont faites avant fork.
4. `fchdir` ancre le lookup, mais ne ferme pas la substitution du basename par le propriétaire root/contrôle ou un ACL writer après sa dernière vérification. Cette limite reste explicite. Les acteurs group-write des ancêtres ne peuvent plus rediriger le lookup d'exécution en renommant un chemin déjà ancré.
5. La liaison d'invocation doit déclarer ce cwd et la nouvelle preuve du répertoire. Le fingerprint moteur des handles reste inchangé ; le fingerprint d'invocation évolue. Ne pas réinterpréter silencieusement un ancien cwd='/' comme cwd leaf. Le contrat/migration doit refuser ou réapprouver explicitement ces anciennes liaisons si elles sont déjà persistées.

## ACL et droits

Sur macOS, mode022=0 ne suffit pas : une ACL étendue peut accorder des droits de modification à un autre principal. **Politique simple recommandée pour ce lot : refuser toute ACL étendue non vide sur leaf dir et fichier**, sans essayer de calculer les groupes/ordres/flags d'héritage. Inspection par FD `acl_get_fd_np(fd, ACL_TYPE_EXTENDED)` ; erreur d'inspection = refus. libc crate actuel n'expose pas les API ACL Darwin : FFI à déclarer et à tester. Attention, Darwin `acl_get_entry` retourne0 lorsqu'une entrée a été obtenue, et -1/EINVAL à la fin ; ne pas copier la convention Linux1/0. Cette politique conservatrice peut refuser des installations avec ACL de lecture/deny inoffensives ; le diagnostic doit l'expliquer sans mutation des ACL.

Sur Linux POSIX ACL, les bits de groupe reflètent le mask limitant les named users/groups ; mode022=0 exclut leur write effectif. Cela ne qualifie pas automatiquement ACL NFSv4/SMB/FUSE ni un filesystem distant. Définir et tester la portée supportée ; pas de succès silencieux si l'inspection requise échoue. La recherche détaillée du reviewer est dans /tmp/ai-guard-invocation-acl-read-bounds.md.

Stat local sans exécution : /opt/homebrew/Cellar/docker/29.2.1/bin est louis:admin755 ; son docker555. /Applications/Docker.app/Contents/Resources/bin est louis:staff755 ; docker755. La règle leaf022=0 reste donc compatible avec ces installations observées malgré leurs ancêtres admin775. Les ACL de ces feuilles doivent encore être inspectées lors de l'implémentation.

## PATH helpers

**Limiter à `/usr/bin:/bin:/usr/sbin:/sbin`, répertoires système vérifiés root et sans022 ; retirer Homebrew, /usr/local/bin et /opt/podman/bin du PATH helpers.** Un PATH statique contenant un répertoire admin775 réintroduit une autorité admin pour docker-credential* même si le CLI principal est ancré. L'ancrage du principal ne corrige pas ces lookups internes.

Ne pas élargir cette règle automatiquement pour préserver un helper. Les plugins/configurations de l'OS account restent dans la frontière de contrôle explicitement documentée ; aucune fermeture générale du chargement natif n'est revendiquée. Si Compose Desktop a concrètement besoin d'un credential helper absent, qualifier l'échec réel et produire un diagnostic exploitable ; ne pas réintroduire gid80 par exception implicite. Les images/plugins/credentials ne sont pas modifiés par cette étude.

## Bornage de la lecture SHA256

Recommandation précise : **maximum512MiB pour un CLI**, rejet de `metadata.len()>cap` avant lecture. Les CLI Docker observés mesurent26949762 et43052752octets ; le cap conserve une marge sans promettre toutes installations futures.

Streamer en chunks64KiB en lisant au plus `initial_len+1` octets (`Read::take` ou compteur équivalent) ; exiger `total==initial_len`. Toute donnée supplémentaire, EOF prématuré, identité/len/mode/owner/mtime/ctime finale différente refuse l'approbation. Une croissance concurrente sans EOF ne peut plus prolonger la boucle au-delà du cap+1. Utiliser checked_add pour la borne, même si le cap interdit l'overflow.

Cette borne garantit un nombre d'octets fini, pas un délai mur sur un read bloqué d'un filesystem distant. O_NONBLOCK traite le FIFO/non-regular avant lecture ; il n'offre pas de timeout fiable aux fichiers réguliers NFS/FUSE. Ne pas présenter ce cap comme une fermeture de toute disponibilité disque.

## Sources primaires

- SDK Apple local, usr/include/unistd.h:564 : fchdir ; aucune déclaration fexecve/execveat trouvée. usr/include/spawn.h:75/187 : posix_spawn_file_actions_addfchdir et alias _np déprécié. libSystem.tbd/libsystem_kernel.tbd exposent fchdir, execve et addfchdir ; aucun symbole fexecve/execveat trouvé. Ces observations concernent ce SDK, pas une preuve universelle pour toutes versions macOS.
- SDK Apple local, usr/include/sys/acl.h : ACL_TYPE_EXTENDED et acl_get_fd_np.
- Apple, acl_get_entry et convention de retour : https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/acl_get_entry.3.html
- Linux man-pages, fexecve : https://man7.org/linux/man-pages/man3/fexecve.3.html ; execveat : https://man7.org/linux/man-pages/man2/execveat.2.html. Linux permet le choix par FD et explique que les modifications de contenu restent une question de permissions.

Statuts : étude/test natif Darwin PASS pour ancrage et constat cwd ; /dev/fd FAIL EACCES ; Linux NOT_RUN ; Docker/Podman/Compose réels NOT_RUN ; aucune qualification production acquise. Implémentation et mise à jour du contrat attendent la décision de l'orchestrateur.
