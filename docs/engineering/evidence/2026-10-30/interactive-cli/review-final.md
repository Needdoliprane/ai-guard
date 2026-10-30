# Revue ciblée du candidat CLI TTY — 30 octobre 2026

Revue indépendante en lecture seule, mode conseil du skill `security-audit`. Ce rapport porte sur le delta entre `/tmp/ai-guard-engine-logs-final.mGhUZD/source` et `/tmp/ai-guard-interactive-review.2t51Ge`. Il ne constitue ni l'audit global Daybreak, ni une qualification moteur/fournisseur/release. Aucun programme du projet, test, moteur ou service externe n'a été exécuté par le relecteur. Aucun secret réel ouvert. Le seul fichier créé est ce rapport demandé.

Empreintes vérifiées du candidat :

- `src/runtime/terminal.rs`: `c065e5cc51cacdd95108ac8adda0ea635e4c64ca766749a770a321af6b39e117`.
- `src/runtime.rs`: `d6b6d20ec353870f35d709a405a34fdff0d285cbe7b6e66bbba10d431fcf09fa`.
- `Cargo.toml` et `Cargo.lock` identiques à la base (diff exit 0).

AGENTS.md, RTK.md, contrat d'architecture, règles de code, gates, stratégie de tests et les deux rapports de réalisation/tests demandés ont été lus. Le skill projet `.agents/skills/ai-guard-security-review/SKILL.md` est absent ; son chargement automatique n'est pas confirmé. La revue suit les contrats et le skill de sécurité général en mode conseil.

## Résultat

**FAIL de revue sur le gel : deux défauts P2 confirmés par lecture.** Aucun P0/P1 établi dans cette revue limitée. Les corrections et leurs régressions restent à relire sur un nouveau gel ; les constats ne sont pas une affirmation d'exploitation observée sur moteur.

## TTY-01 — P2 — La fin du CLI moteur abandonne les sorties en attente et peut effacer un refus du filtre

**Source du snapshot :** `src/runtime.rs:1039–1042`, `1035–1067`, `1070–1076`, `1126`; `src/runtime/terminal.rs:653–656`, `715–723`. Le runner ne considère l'exécution comme échouée au titre du transport que lorsque `transport_stop_reason` existe (`src/runner/managed.rs:1919–1921`).

**Préconditions :** exécution TTY admise ; processus attach terminé alors que des octets attendent encore dans le master PTY, ou que le pump est suspendu avant de traiter sa prochaine sortie. Le worker peut terminer normalement avec exit 0. Pas de droit au contrôle requis pour produire la sortie : le worker est la source moins fiable.

**Démonstration source :** `child.wait()` et `pump` sont deux branches concurrentes. La branche wait quitte immédiatement la boucle, puis la portée du pump se termine sans drain. Une inspection terminale réussie conserve alors `transport_issue=None`. La queue PTY peut contenir des diagnostics utiles ou une séquence qui dépasserait les bornes du filtre ; celle-ci n'est jamais examinée. Le résultat devient un succès si le worker a exit 0. Ce n'est pas une fuite brute : les octets abandonnés ne sont pas affichés. C'est une perte de sortie et de vérité du statut de transport.

La branche initiale d'inspection/audit (`runtime.rs:1059–1061`) renforce une séquence déterministe de test : pendant qu'elle attend, le pump n'est pas pollé. Le processus attach peut produire un suffixe puis terminer ; au retour, wait peut gagner. Le batch historique lançait ses lecteurs séparément et les attendait après le wait ; il n'avait pas cette perte précise de queue TTY.

**Point nécessaire dans le correctif :** le `Command` du parent conserve trois descripteurs slave après spawn. `TerminalSession::spawn(&mut command)` les installe via `Stdio::from(File)` ; le `command` reste vivant dans `execute_interactive`. La source officielle Rust montre que `Command` possède les trois `Option<Stdio>`, que `setup_io(&self)` les emprunte et que `Stdio::Fd` garde le FD parent au profit d'un `ChildStdio::Explicit` : [source Rust, setup_io et Stdio](https://doc.rust-lang.org/src/std/sys/process/unix/common.rs.html#357-419). Tokio appelle le spawn standard puis construit le Child sans prendre ces Stdio (source locale `tokio/src/process/mod.rs`, `build_child(&self)`). Un drain attendant EOF sans fermer ces copies parent risque donc de ne jamais finir. Faire `drop(command)` après le spawn réussi, puis attendre séparément fin du processus et fin/drain borné du transport. Garder les erreurs de filtre et de drain dans le résultat. Ne pas écrire de sortie après la restauration finale du terminal. Terminer explicitement le filtre pour masquer les préfixes retenus, sans flush brut.

**Régression nécessaire (NOT_RUN ici) :** vrai PTY local et CLI fixture Rust, sortie finale sûre avec sentinelle non secrète, fin immédiate, callback d'observation volontairement retenu puis relâché ; vérifier la sentinelle complète. Variante avec queue CSI trop longue puis exit 0 : résultat transport en échec même si observation métier exit 0. Vérifier EOF après fermeture du processus, absence de slave détenu par Command, délai de drain borné, et canary littéral masqué. Un test qui constate seulement exit 0 ou absence d'ESC ne suffit pas.

## TTY-02 — P2 — Les marges de défilement autorisées ne sont pas réinitialisées à la sortie

**Source du snapshot :** `src/runtime/terminal.rs:397` autorise CSI `r`; `:20` définit `PRESENTATION_RESET`; `:463–495` écrit exclusivement cette séquence pour la présentation.

**Préconditions :** terminal compatible xterm/VT100 ; une sortie worker autorisée atteint le terminal physique avant sa fin. Aucun privilège additionnel requis.

**Séquence minimale :** le worker écrit `ESC[?1049l ESC[2;3r` (sans les espaces séparateurs), puis termine après que le pump a transmis ces octets. Les deux contrôles sont autorisés. Le premier garantit l'écran normal ; le second limite la zone de défilement aux lignes 2–3. À la sortie, la remise à zéro contient bracketed paste off, alternative screen off, curseur visible et SGR reset, mais pas CSI `r`. Une désactivation d'écran alternatif alors que l'écran normal est déjà actif ne réinitialise pas ces marges. `tcsetattr` restaure les attributs du périphérique, pas cet état de l'émulateur.

**Impact :** le worker modifie durablement la présentation du terminal du mainteneur après restitution au shell ; sorties et navigation se retrouvent dans une zone réduite, jusqu'à reset manuel. C'est un défaut de restauration de frontière worker → terminal hôte, limité à cette session, pas une exécution de code hôte. La [documentation officielle xterm](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html) définit DECSTBM comme le réglage haut/bas de la zone et sa forme par défaut comme la fenêtre entière.

**Correction minimale :** ajouter une réinitialisation explicite des marges après le retour à l'écran normal, et inventorier les états persistants admis par la allowlist pour assurer la symétrie de sortie. Ne pas remplacer cela par RIS global sans évaluer ses autres effets.

**Régression nécessaire (NOT_RUN ici) :** vérifier que le filtre transmet la séquence admise, puis que la restauration remet les marges à la hauteur complète dans un terminal/interpréteur de contrôle. Le test doit examiner la présentation séparément de termios, pour sortie normale, erreur et annulation. Une comparaison termios seule ne détecte pas ce défaut.

## Risque complémentaire à reproduire, sans nouvelle sévérité

`runtime.rs:1059–1061` attend l'inspection puis `on_started` dans une branche de select ; le pump, les signaux et le deadline de ce select ne sont plus pollés pendant ces awaits. `runner/managed.rs:1537–1552` inclut l'enregistrement durable puis `post_event`, et le client HTTP porte un timeout de 15 s (`runner.rs:152–154`). Un ACK lent peut donc figer l'interaction et retarder le traitement de Ctrl-X / signal. La même structure de timeout existait dans la boucle batch de la base ; ne pas requalifier ce défaut historique de scheduling comme nouveau timeout autonome livré. Tester spécifiquement la réactivité TTY sous ACK retardé et vérifier le délai annoncé avant d'affirmer que pump/lifecycle sont indépendants.

## Chemins examinés sans défaut concret établi

- Mode optionnel omis du JSON batch ; fonction de hash et ordre des champs historiques conservés ; enums fermées. Lien mode/preset dans l'admission, refus capture/TERM, refus nonTTY en tête de `choose_plan`.
- Audit `security.decision` durable avec mode présent uniquement si demandé ; lookup/reconstruction typés ; null/variante inconnue dans l'historique refusés. Événement attach autorisé ACK avant claim et start. Un échec de callback avant start empêche ce dernier ; après start le chemin d'erreur tente stop et conserve la preuve.
- Claim consommé non réarmé et reattach explicitement refusé ; aucun second start/restart/exec ajouté pour la reprise.
- Les trois flux du CLI moteur vont au slave filtré, stderr compris. Pas de transcript ajouté, pas de capture TTY ni parseur fournisseur ; compteurs séparés rendus null. Dictionnaire non Debug, littéraux connus retenus entre chunks, allowlist et bornes visibles.
- Création TTY liée au preset V3, TERM fixe inspecté et fingerprinté, autres valeurs Env exclues du DTO/digest. Log-driver none et restrictions existantes conservés. Le basniveau public refuse le preset interactif.
- Guard de restauration avant diagnostics et Drop ; enfant dans sa propre session ; stop ciblé sur CID. L'annulation tente un stop async sans finaliser quota/audit, uniquement si executor vivant. Aucune garantie SIGKILL déduite.

Ces observations sont des conclusions de lecture, pas des PASS moteurs. Les concessions StdinOnce=true Docker et refus de reattach sont évaluées telles qu'explicitement approuvées, sans les transformer en défauts nouveaux.

## Preuves et limites

- **PASS transmis par Luna/root, non réexécuté par le relecteur :** logpolicy 4, audit 3, auth 8, registry 36, managed 14, recovery 16 ; retest terminal 8 et garde basniveau 1. Les deux FAIL initiaux termios restent dans leurs rapports ; la correction du helper lit le readback accepté et ne masque pas PENDIN.
- **NOT_RUN par cette revue :** tous les tests, moteurs, fournisseurs et effets de terminal. Les recherches, diffs, lectures et empreintes ci-dessus ont été effectués seulement sur sources/rapports autorisés.
- **HORS REVUE :** nouveaux `tests/runtime_interactive.rs` et `tests/runtime_fixtures/interactive*` en reprise par Luna/root, absents du gel contractuel de cette revue ; aucune approbation de ces tests.
- **Limites préexistantes/ouvertes :** timeout autonome après SIGKILL broker, reattach après claim, vraie TUI Codex/Claude, Podman/Linux/autres architectures, qualification cloud/PKI. Aucun gate production fermé par ce rapport.
- **Blocages avant acceptation du lot :** traiter TTY-01 et TTY-02, exécuter leurs régressions ciblées, relire les changements ; poursuivre ensuite les preuves moteur sans extrapoler les PASS logiciels.

## Addendum — relecture du correctif figé

Nouveau candidat : `/tmp/ai-guard-interactive-review-fix.0RyOPH`, comparé au premier gel. Empreintes vérifiées : `src/runtime.rs` = `f9e812c7d55809693a569aee0c2caadb703dfaa8dcb948623d275cebc14a29da` ; `src/runtime/terminal.rs` = `380e9b3ba973fc22a6724c9e5463918c2601c56d03143eece80c75a8c3ce6518`. Le diff récursif de `src` confirme que seuls ces deux fichiers de production changent. Rapport d'implémentation mis à jour lu. Aucun test ni moteur exécuté par le relecteur.

**État initial de la relecture source :** causes apparentes de TTY-01 et TTY-02 traitées par le delta ; conclusion conditionnée aux tests. **Cette acceptation source est maintenant contredite pour TTY-01 par les preuves comportementales Luna ci-dessous : nouveau gel FAIL, acceptation du drain BLOCKED.** Le FAIL du premier gel reste inchangé.

- **TTY-01, raccords source présents mais résultat FAIL :** `runtime.rs:1043–1048` ne quitte plus sur `child.wait` ; il signale le drain et conserve le pump. `terminal.rs:654–685` consomme le builder puis `drop(command)` ferme ses copies slave. Les branches EOF/EIO passent par `filter.finish()?` avant de terminer (`:747–755`) ; les préfixes privés sont masqués et un contrôle incomplet renvoie une erreur si le filtre reçoit ces octets. Les limites d'octets et d'attente donnent une erreur nommée, pas un faux EOF. `TransportIssue::Failure` n'est pas neutralisé par une observation worker exit 0. La branche d'entrée n'envoie plus au slave dès que le drain est signalé et une écriture précédemment bloquée peut être annulée par le watch. La future pump est locale et détruite avant `session.restore()` (`runtime.rs:1078–1080`) : aucune tâche d'écriture détachée n'est laissée après restauration. L'annulation de la future conserve les Drop existants du guard, de l'enfant et de la tentative d'arrêt ciblée. Ces propriétés ne compensent pas une queue perdue par l'OS avant sa lecture : les nouveaux tests démontrent précisément cette absence de résultat.
- **TTY-02, correction source acceptée :** `PRESENTATION_RESET` contient désormais CSI `r` immédiatement après la sortie d'écran alternatif (`terminal.rs:22`). La remise à défaut porte donc sur l'écran normal et couvre l'état persistant identifié.
- **Régressions relues, NOT_RUN ici :** deux tests utilisent réellement `spawn_transport`, réapent le processus avant de poller le pump et exigent un EOF dans le délai ; ils vérifient queue de sortie, masquage du suffixe privé, CSI surdimensionné et contrôle incomplet. Ils couvrent bien le bug des copies slave et le filtrage d'une queue post-exit. Le test de reset inspecte la séquence émise, sans émulation de terminal ; les tests existants du guard portent aussi cette constante. Ces tests ne constituent pas une exécution de la boucle complète `execute_interactive`, ni une preuve Docker/Podman. Les nouveaux tests moteur `runtime_interactive` restent hors revue.

**Réserve de timing maintenue :** les signaux, le deadline et le drain sont des branches du select, mais `runtime.rs:1070–1072` attend toujours `execution_inspect` et `on_started` dans le corps d'une branche. Pendant ces awaits, aucune autre branche ni le pump n'est pollée. La nouvelle limite de drain 3 s n'est donc pas une garantie murale stricte si le callback d'audit est lent ; la même réserve de scheduling historique était déjà relevée ci-dessus. Le rapport d'implémentation doit éviter de présenter cette sélection comme entièrement indépendante. À reproduire avec un ACK retardé ; cela ne rouvre pas les deux causes précises corrigées et n'est pas une qualification du timeout autonome absent.

**Suite requise :** recueillir les résultats Luna correspondant exactement aux deux empreintes nouvelles, puis les preuves moteur ciblées. Cette relecture ne transforme aucun NOT_RUN en PASS et ne clôt aucun gate de release.

### Résultats Luna reçus après la relecture source

Lecture effectuée de `/tmp/ai-guard-interactive-final-terminal.log` : **9 PASS, 2 FAIL**, aucun ignoré. `presentation_reset_restores_authorized_scroll_region` PASS. `reaped_transport_drains_actual_pty_tail_until_real_eof_and_masks_private_suffix` FAIL : sortie vide `[]` au lieu du suffixe attendu. `reaped_transport_cannot_hide_a_filter_failure_or_incomplete_control_in_its_tail` FAIL : `RuntimeStreamEnded` alors qu'une erreur était requise. Aucun test n'a été relancé par le relecteur.

**Verdict actuel : TTY-01 reste ouvert P2 ; correctif de drain non accepté, BLOCKED avant moteur. TTY-02 traité par source et régression ciblée PASS transmise.** Root transmet aussi fmt/release PASS et deux avertissements Clippy à corriger ; cela ne change pas les FAIL comportementaux. La piste de perte de queue à fermeture du dernier slave Darwin doit être établie puis corrigée, sans affaiblir les assertions. La conservation explicite d'un slave jusqu'au drainage constaté est une piste, pas une correction déjà relue ou validée.

### Investigation Darwin en lecture seule

Les sources officielles Apple XNU `main` consultées établissent deux chemins pertinents : `ptsclose` appelle `ttyclose` puis notifie le côté master ; `ttyclose` vide les files lecture/écriture. Cela est cohérent avec une queue perdue à la fermeture du dernier slave. [XNU tty_dev.c, ptsclose](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/tty_dev.c), [XNU tty.c, ttyclose](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/tty.c).

**Un keeper seul ne peut pas encore être considéré suffisant.** À la sortie du leader de session, XNU traite son terminal de contrôle et appelle `VNOP_REVOKE(..., REVOKEALL, ...)`, après `ttywait`. Le transport courant fait précisément `setsid` puis `TIOCSCTTY`. Une copie slave conservée par le parent risque donc d'être également révoquée. Il s'agit d'une inférence fondée sur cette chaîne source, à valider sur le Darwin testé ; je n'ai pas identifié le commit XNU correspondant exactement à son noyau ni exécuté la reproduction. [XNU kern_exit.c, lignes 2093–2140 de la version consultée](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/kern_exit.c).

Contraintes de revue pour la proposition keeper :

- Garder un seul FD explicite, possédé et CLOEXEC, avec Drop sur toutes les sorties ; consommer/détruire Command demeure nécessaire pour éliminer ses copies cachées.
- Ne jamais transformer EAGAIN en EOF. Après observation de fin du processus, drainer les octets disponibles jusqu'à EAGAIN, fermer le keeper, puis exiger une vraie fin de flux et terminer le filtre. Un descendant qui conserve ou continue d'écrire le slave doit conduire à la borne/erreur prévue, pas à une attente infinie ou une réussite simulée.
- Les limites octets/temps doivent inclure la phase avant fermeture du keeper, les contrôles incomplets et les écritures physiques ; annulation/erreur ne doivent ni détacher un écrivain ni laisser le terminal raw. La réserve de polling des callbacks d'audit reste applicable.
- Conserver les assertions qui échouent : processus terminé avant le premier poll, suffixe exact non vide, masquage et erreur effective du filtre. Retester la chaîne réelle `setsid`/terminal de contrôle ; un test simplifié retirant cette chaîne ne prouve pas la correction du produit.
- Toute adaptation du terminal de contrôle doit requalifier le comportement effectif de SIGWINCH/resize, Ctrl-C, signaux externes et isolation du groupe du shell. Aucune de ces garanties ne se déduit du seul isatty des trois descripteurs.

## Verdict du dernier gel Darwin

Candidat relu : `/tmp/ai-guard-interactive-kernel-review.VDn65T`, comparé à `fix.0RyOPH`. Empreintes vérifiées :

- `src/runtime/terminal.rs` : `851a9612118db27bf75ab89f8b05bc9c3fb7fb12800867ceddc146ad62d0e980`.
- `src/runtime.rs` : `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`.
- `src/runtime/managed.rs` : `411dd20cee645fcc80003b600e33ac55d1f8af5c9f00bfb483012e610d6d028a`.

**PASS de la relecture ciblée des deux correctifs sur ce gel, avec preuves logicielles Luna ; aucun bloqueur nouveau établi avant la qualification Docker ciblée.** TTY-01 est maintenant traité pour le scénario Darwin reproduit. TTY-02 reste traité. Les FAIL des deux gels antérieurs sont conservés et ne sont pas réétiquetés PASS. Cette conclusion n'est ni un PASS moteur ni une qualification de compatibilité Docker/Podman/fournisseur.

Le correctif conserve `setsid` mais retire `TIOCSCTTY`, ce qui évite le chemin de révocation du terminal de contrôle identifié dans XNU. Les trois flux du client moteur restent sur le PTY. Le keeper est un `Option<File>` explicite dans `TerminalSession`, fermé après lectures nonbloquantes jusqu'à EAGAIN une fois le processus réapé ; EOF/EIO reste ensuite une étape distincte, avec clôture stricte du filtre. Le builder Command est toujours consommé puis détruit. Les retours d'erreur et l'annulation détruisent les FD possédés ; aucun écrivain détaché n'est créé, le pump est abandonné avant restauration. Le compteur de drain et les erreurs du filtre demeurent conservés.

SIGWINCH est envoyé uniquement au PID du Child possédé, avant drain. Dans la boucle courante, `child.wait()` et le pump sont pollés dans la même tâche ; dès que wait rend son résultat, le watch est positionné sans await avant toute reprise du pump. La branche resize revérifie ce watch avant son ioctl/kill, sans await entre cette vérification et le signal. La source locale de Tokio 1.54.1 (`src/process/unix/reap.rs:68–100`, `src/process/mod.rs:1382–1397`) confirme que le Child vivant est réapé lors du poll de wait ; la file de réap en arrière-plan traite les orphelins après Drop. Aucun chemin où un PID réutilisé serait signalé n'a été établi ici.

Les seules modifications de `runtime.rs` et `runtime/managed.rs` depuis le gel précédent remplacent `1 | 2 | 3` par `1..=3` ; la comparaison des sources ne montre pas d'autre changement de contrat dans ces fichiers.

**Preuves exécutées lues :** `/tmp/ai-guard-interactive-kernel-fixed-terminal.log` donne **11 PASS, 0 FAIL, 0 ignoré**. Le test conserve l'exigence de réaper le producteur, attendre 20 ms, puis seulement poller le pump ; il vérifie aussi isatty et absence de session de contrôle sur le PTY. La queue attendue est non vide, son suffixe privé est masqué, CSI trop long et contrôle incomplet produisent effectivement les erreurs requises. `/tmp/ai-guard-interactive-kernel-fixed-release-retry.log` donne build release PASS. Le défaut d'invocation antérieur et les passes Clippy transmises par root ne sont pas réinterprétés comme des tests exécutés par ce relecteur ; Clippy all-targets est encore en passe finale au moment de cette rédaction.

**Limites maintenues :** le harnais `runtime_interactive` n'est pas approuvé par cette revue ; aucun moteur n'a été lancé par le relecteur. L'absence de controlling TTY du client moteur exige maintenant les preuves Docker réelles de resize, Ctrl-C distant, signaux/EOF/stop, restitution du terminal et autre worker intact. La réserve sur les callbacks d'audit lents reste inchangée : pas de garantie murale stricte de 3 s pendant ces awaits. Les limites SIGKILL/reattach/fournisseurs/plateformes et les gates ouverts restent applicables.

## Relecture de l'accès au terminal physique

Candidat `/tmp/ai-guard-interactive-device-review.TPsnmH`, comparé à `kernel-review.VDn65T`. Diff récursif `src` : seul `runtime/terminal.rs` change. SHA256 vérifié : `5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`.

**Verdict borné : correction source acceptée pour le chemin CLI avec runtime I/O actif ; aucun nouveau bloqueur de sécurité établi. Résultats Luna terminal12/auth8/fmt/Clippy/release en attente au moment de cette relecture.** Aucun test ni moteur exécuté par le relecteur.

Le premier essai Docker reste **FAIL**, rapporté par root : inscription kqueue de l'alias `/dev/tty` en EINVAL avant start, conteneur Created, claim consommé, quota conservé, audit de 15 événements valide, zéro usage terminal ; aucun rejeu n'est autorisé par cet échec. Ce résultat moteur n'est pas remplacé par l'acceptation source ci-dessus. La sonde et la régression pré-correction rapportées par root/Luna établissent alias EINVAL versus périphérique réel utilisable ; leurs tests ne sont pas attribués à cette revue.

Points examinés dans le delta :

- `/dev/tty` sert à attester session et premier plan, pas à l'AsyncFd. Les trois flux standard doivent être TTY, de même SID que le processus et dans son groupe de premier plan (`terminal.rs:431–434`, `503–512`).
- Le candidat ouvert provient du nom de périphérique d'un flux standard. Il est absolu, sous `/dev`, sans composant parent ; l'alias indirect est écarté. L'ouverture possède ses propres flags O_NONBLOCK/NOCTTY/NOFOLLOW/CLOEXEC. Le duplicata de référence ne sert qu'à fstat ; aucun F_SETFL ne modifie les flags partagés de stdin/stdout/stderr.
- Le FD ouvert doit être un périphérique caractère avec dev/inode/rdev identiques à la référence retenue (`:487–494`). L'identité ouverte et `/dev/tty` subissent ensuite une nouvelle vérification SID/premier plan. Une substitution vers un autre périphérique entre résolution et ouverture est donc refusée par l'identité ou la réattestation. Les FD restent possédés par File/AsyncFd et sont fermés lors des erreurs ; aucun chemin TTY ou contenu d'environnement n'est ajouté aux erreurs.
- Le préflight readiness se déroule dans `choose_plan` via la validation existante avant admission/import ; il ne modifie pas termios. `TerminalGuard::open` revalide ensuite et réouvre un FD propre pour l'usage réel. Le scénario fd0 alias peut sélectionner fd1/fd2 attesté ; trois alias sont refusés. Les contrats tenant, audit, credentials et launch n'ont pas changé.
- La nouvelle régression relue utilise le véritable `TerminalGuard::open` dans un processus possédant un terminal de contrôle ; cas direct, alias stdin et trois alias, avec contrôle de nonmodification des flags stdin. Le driver/IPC de `runtime_interactive` modifié par root/Luna reste hors approbation de cette revue.

**Précondition API précise :** `Handle::try_current()` (`:525–528`) établit seulement la présence d'un runtime Tokio. Un runtime construit sans `enable_io` peut passer ce garde puis paniquer dans `AsyncFd::register`, même avec un vrai TTY. La source locale Tokio 1.54.1 `runtime/io/registration.rs:78` appelle `driver().io()`, dont `runtime/driver.rs:100–103` fait un `expect` lorsque I/O est désactivé. La fonction annonce déjà qu'elle exige un runtime I/O actif et le CLI utilise `#[tokio::main]` (`src/main.rs:174`) ; ce scénario reste une violation de précondition par un appelant bibliothèque, pas un bloqueur du CLI Docker ni un nouveau P2 de frontière de sécurité. Ne pas présenter `try_current` comme une vérification complète du reactor ou une garantie générale sans panic.

Le moment exact du passage en arrière-plan ne peut pas être verrouillé par ces seules vérifications ; le contrat porte sur les vérifications répétées du FD possédé et les erreurs/signaux du terminal. Aucun contrôle détenu par un worker ne lui permet de modifier les FD ou la session du broker dans le périmètre examiné. Les réserves de timing et les qualifications moteur restantes demeurent ouvertes.

## Recoupe finale des preuves Docker et Codex

Relecture ciblée des rapports `/tmp/ai-guard-interactive-real-test.md`, `/tmp/ai-guard-codex-tui-real.md`, `/tmp/ai-guard-codex-tui-keyboard.md`, du log scalaire brut raw-keys, et des assertions actuelles `tests/runtime_interactive.rs` / `tests/runtime_fixtures/interactive_probe.rs`. Aucun test, fournisseur, E2E, campagne globale ou lecture de credential exécuté par le relecteur.

Les empreintes des deux fichiers de production restent celles du gel qualifié hO9 : terminal `5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`, runtime `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`. Empreintes des tests relus : `runtime_interactive.rs` = `78e18be1a808265dc95579b9d65549731d7d44e7aa742cc7a4222dbd6381d0f6`, helper = `9f4cb01878a24d0edbb6a7df91c525e376396007876a666decce2ffbd6c60860`. Les deux premiers PASS Docker rapportés ont été exécutés avec la version antérieure du test `188377...`, avant l'ajout raw-keys ; ne pas attribuer rétrospectivement ce dernier hash à ces runs.

### Valeur des assertions

- **Restauration hôte : preuve pertinente et non réécrite par cleanup.** `run_on_pty` lit termios après statut broker et drain mais avant `ACK` au driver (`tests/runtime_interactive.rs:669–684`). Le driver garde la session de contrôle vivante jusque-là. L'erreur éventuelle est mémorisée puis fait échouer le test après libération du driver ; `HostPty::drop` restaure uniquement ensuite le PTY privé. Le masque Darwin porte sur PENDIN ; ICANON/ECHO/ISIG, les autres flags comparés et les caractères ne sont pas neutralisés. Cela prouve les champs termios comparés, pas tous les états visuels d'un émulateur (marges, couleurs, écran alternatif).
- **Deux ACK distincts.** L'ACK du driver n'est pas l'ACK terminal du gateway. Pour ce dernier, le middleware inspecte une whitelist Docker avant de transmettre l'observation au vrai gateway (`:815–835`), conserve succès/échec de l'inspection en mémoire et n'altère ni transition ni réponse. Une absence/erreur d'inspection fait échouer les assertions. Le test vérifie ensuite quota et audit réouvert ; il ne prétend pas inspecter un conteneur déjà supprimé par cleanup.
- **Filtre : preuve non vacuë du cas préparé.** Le helper écrit SAFE_BEFORE, les séquences adversariales, SAFE_AFTER, puis attend une ligne `exit`. Le harnais exige le marqueur final et l'absence des quatre familles préparées avant d'envoyer cette ligne (`:910–925`). Les deux marqueurs et l'ACK clavier/resize sont revérifiés à la fin. Ce n'est ni une preuve de tout l'espace des séquences ANSI ni une preuve générale de DLP ; les tests unitaires de partitions et littéraux restent distincts.
- **Raw-keys : vraie observation distante.** Le helper passe son TTY en raw sans echo, lit huit octets et imprime leur hex calculé (`interactive_probe.rs:43–73`). Le test exige exactement CR/LF/0x03/UTF-8 é/flèche haut, puis une sentinelle et exit 0 (`runtime_interactive.rs:1073–1105`). Le marqueur hex n'est pas la simple réémission locale des octets envoyés. Cela établit le transport exact de ces touches, y compris le byte Ctrl-C ; pas le comportement d'interruption d'un programme distant en mode canonique, ni la soumission de message Codex.

### Écart de preuve Ctrl-X à conserver

Le cas nommé `docker_interactive_double_ctrl_x_stops_only_the_owned_worker` envoie bien deux 0x18 espacés, mais ses assertions (`:1234–1288`) n'exigent ni `timed_out=false`, ni le motif durable `operator requested ... Ctrl-X Ctrl-X`, ni une échéance distinguée du timeout worker de 120 s. Un transport ignorant les touches puis s'arrêtant au timeout pourrait satisfaire les mêmes assertions : CLI nonzero, worker arrêté, quota libéré, une autorisation d'attachement. C'est un défaut de discrimination du test, pas une démonstration d'un défaut actuel de l'arrêt produit. Le run rapporté en 13,90 s est cohérent avec l'arrêt demandé bien avant 120 s, mais le test seul ne verrouille pas cette régression. Recommandation ciblée : vérifier motif et absence de timeout dans les données durables, sans réinterpréter un booléen ou un exit moteur comme preuve du motif.

L'absence de second worker témoin est explicitement conservée : ce test ne prouve pas « seul le worker possédé est arrêté » face à un concurrent. Le nombre d'autorisations d'attachement ne constitue pas à lui seul un compteur de commandes moteur effectivement émises. Aucune nouvelle campagne n'est demandée ou lancée par cette revue.

### Résultats pouvant être cités

- **PASS rapporté Docker exit7**, session fraîche en 12,89 s : clavier bidirectionnel, isatty, taille 80×24 puis 120×40 observée, filtrage préparé, véritable exit 7, restauration contrôlée avant ACK driver, quota libéré, allocation observée non facturable et audit réouvert. Sortie Cargo originelle disponible dans l'historique outil selon rapport, pas dans une archive brute dédiée.
- **PASS rapporté Docker Ctrl-X**, 13,90 s : arrêt et finalisation du worker observés ; portée et lacunes du motif/témoin ci-dessus. Le fichier `ctrlx-cargo.log` est un résumé rédigé, pas un log brut.
- **PASS Docker raw-keys désormais final**, 12,29 s : le rapport s'est enrichi pendant cette revue ; `/tmp/ai-guard-interactive-rawkeys-cargo.log` a été lu et porte 1 PASS, 0 FAIL, 0 ignoré, broker exit 0, termios PASS avant ACK, driver terminé. Cela remplace le NOT_RUN de préparation de ce cas seulement. Le log ne contient pas de transcript PTY. Le périmètre est Darwin 27.2.0 arm64, Rust 1.99.1, Docker client 29.2.1 / serveur 29.9.2 ; aucune extrapolation Podman/Linux.
- **Codex réel : premier tour et export partiel attestés par rapport, critère deux tours FAIL/INCOMPLETE conservé.** Le fichier exporté exact d'une ligne et son hash appuient l'effet du premier tour ; le second prompt sans transition claire, puis stop exit137, ne devient pas un succès de conversation. Le modèle affiché, le provider configuré et le succès du premier tour n'attestent pas indépendamment le compte/modèle effectivement servi.
- **Diagnostic clavier Codex offline : PASS local borné par rapport.** `/status`, `/warnings` et `/exit` avec texte puis CR distinct, réseau none, sans credentials et token usage affiché zéro : preuve d'interaction avec le vrai client, pas de tour modèle ni d'auth fournisseur. Le message de mode embedded n'établit pas un échec d'auth. Aucun remplacement générique LF→CR n'est justifié par cette observation.
- **Nouvelle conversation Codex à deux tours : aucun PASS accordé par cette revue.** Elle était en cours lors de l'assignation ; aucun rapport final de cette nouvelle session n'a été relu ici.

### Limites résiduelles

Le plafond de capture 2 MiB fait échouer les résultats incomplets ; les attentes principales portent des deadlines. Toutefois le `pump` du harnais (`runtime_interactive.rs:286–303`) lit jusqu'à EAGAIN sans borne d'itérations interne : une sortie continue peut retarder le retour au contrôle et à ses échéances, même après overflow. Les fixtures observées sont finies et le nouveau raw-keys a en plus un timeout externe ; ne pas présenter le harnais comme garantissant une borne globale sous n'importe quelle charge.

Les probes génériques n'utilisent pas de credential réel ni de canary secret injecté. Log-driver none est inspecté ; cela ne constitue pas à lui seul un scan exhaustif des persistences. Le scanner Codex rapporté ne couvre que quatre fichiers nommés, pas toutes les bases, caches ou sorties possibles. Les réserves runtime I/O bibliothèque, callbacks audit lents, timeout autonome après SIGKILL, reattach consommé, témoins concurrents, autres plateformes et compatibilité fournisseur complète demeurent ouvertes. Aucun gate de production n'est clôturé par cet addendum.

## Clôture ciblée de l'écart d'assertions Ctrl-X

Relecture du test renforcé SHA256 `3c8d7dfd48e1738f177493ccd2d49021ff2b3fed68f0f81775a34b4aee15810b`, de la dernière section du rapport réel et du log original `/tmp/ai-guard-interactive-ctrlx-strict-cargo.log`. Empreintes production/helper revérifiées inchangées. Aucun test relancé par le relecteur.

**PASS : défaut de discrimination des assertions Ctrl-X fermé sur cette nouvelle session.** Le test mesure le temps entre seconde frappe et réception du statut broker, exige `<30 s` (`tests/runtime_interactive.rs:1226–1246`), puis relit après réouverture SQLite un unique `command.failed` avec motif Ctrl-X exact et `timed_out=false` (`:1328–1349`). Il exige aussi worker Terminal, quota libéré et une allocation observée non facturable. Le scénario « touches ignorées puis timeout 120 s » ne satisfait plus ces assertions.

Le log original donne **1 PASS, 0 FAIL, 0 ignoré**, temps total 13,57 s, broker code1, termios PASS avant ACK driver et driver terminé. Le délai propre après seconde frappe est mesuré/asserté mais non imprimé ; aucune valeur exacte n'est inventée. Les anciens résultats restent leurs preuves historiques distinctes.

La clôture concerne uniquement ce défaut de test. **Aucune preuve d'isolation face à un autre worker actif** n'est ajoutée. La réserve sur la boucle interne du pump du harnais et ses deadlines sous sortie continue, ainsi que toutes les limites produit et fournisseur déjà consignées, demeure ouverte.

## Dernière recoupe : flood du harnais, gouvernance et limites Codex

Relecture READONLY du test SHA256 vérifié `a5f30e67a2b3078fbf8efa07fc1836f2f128445765e4371a3e6ea6683529cc4c`, du rapport `/tmp/ai-guard-interactive-harness-flood.md` et de ses logs originaux RED/GREEN. Aucun test ou moteur exécuté par le relecteur.

**PASS : réserve du pump sous sortie continue fermée pour la cause identifiée.** Le pump du harnais utilise désormais un budget de 4 KiB par appel (`tests/runtime_interactive.rs:426–447`) et traite ensuite la socket de contrôle dans le même tour ; la capture reste plafonnée à 2 MiB avec erreur explicite en cas d'overflow. La régression PTY réelle met la sortie et le contrôle en état lisible tout en maintenant un producteur actif. Elle emploie les mêmes branches avec budget64octets/cap16KiB, vérifie traitement du statut, budget initial et overflow, puis arrête le producteur. Les écritures du producteur sont nonbloquantes, les notifications utilisent `try_send`, son drapeau stop est revérifié dans la boucle et `stop_and_join`/Drop assurent son arrêt sur les sorties examinées ; aucun deadlock producteur n'est établi par cette lecture. Le RED échoue explicitement sur dépassement du budget sans la limitation ; le GREEN donne 1 PASS en0,01s. Ce résultat local ne réexécute ni ne remplace les preuves Docker antérieures. La réserve indépendante des awaits d'audit dans le produit reste ouverte : le correctif du harnais ne la ferme pas.

**Gouvernance cohérente avec les nouvelles exigences, sans prétention d'implémentation.** AGENTS.md exige les quatre parcours Codex CLI/Claude Code × abonnement/API, priorité à la sélection explicite, refus d'ambiguïté, aucune bascule silencieuse vers une API facturée, et une vue cloud unique par tenant. `.gitignore` exclut `.env` et `.env.*` avec exceptions limitées aux noms d'exemples ; ces exceptions autorisent des modèles sans valeur réelle, pas des secrets. La liste blanche existante `.dockerignore` n'inclut aucun `.env`. `workspace::excluded_name` (`src/workspace.rs:51–79`) exclut `.env` et `.env.*` après conversion en minuscules, y compris les modèles lors de l'import. `docs/engineering/CURRENT_STATE.md:7`, `IMPLEMENTATION_PLAN.md:11–14` et `PRODUCTION_READINESS.md:22` indiquent expressément que le chargement dynamique, les quatre parcours et l'agrégation cloud ne sont pas livrés/qualifiés par la création du fichier. Le mode0600 et le check-ignore du fichier réel sont des vérifications rapportées par root, pas répétées ni renforcées par cette revue. **Aucun fichier `.env`, valeur de clé ou secret littéral n'a été lu.** Aucun handoff automatique de ses deux clés n'est supposé implémenté.

**Codex deux tours : FAIL de qualification maintenu, attribution de cause inconnue.** `/tmp/ai-guard-codex-tui-two-turns.md` précise désormais qu'une réponse outil tronquée et devenue inaccessible a pu contenir le reçu. L'absence de preuve de visibilité/idle ne démontre donc pas une absence de sortie du client ni un défaut du broker. Le premier effet fichier est exporté/vérifié ; aucun second message ni `/exit` n'a été envoyé. L'expiration300s, exit137 et `timed_out=true`, audit/allocation/quota corrects sont des faits terminaux ; ils ne valident pas une conversation API à deux tours.

**Codex offline-tools : portée locale seulement.** La première tentative reste BLOCKED/NOT_RUN pour ses interactions à cause du CODEX_HOME inexistant. La seconde établit par rapport deux commandes locales `! cat` avec reçus non présents dans les commandes, et ouverture de `/warnings`, sans réseau extérieur ni credential. Elle ne qualifie ni authentification ni tour modèle. `/exit` reste NOT_MET : écran/modal courant ambigu, puis timeout90s ; aucun succès de sortie propre ni bug du broker n'est déduit. Les limites de collecte en mémoire et de visibilité restent indiquées. Aucun des FAIL/NOT_RUN précédents n'est transformé en PASS API, et aucun gate de production n'est fermé.
