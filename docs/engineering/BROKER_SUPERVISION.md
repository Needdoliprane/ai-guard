# Supervision durable du broker — direction approuvée

30 octobre 2026. **Direction structurelle approuvée par le mainteneur : « Implémenter la proposition (recommandé) ». Verticale broker/gardien non encore livrée ; composants de contrôle en cours d'intégration.** L'accord couvre broker et gardien TTY du même binaire Rust, IPC privé, configurations et tests. Il n'installe/charge aucun service OS et n'autorise aucune opération cloud/PKI ou destruction de données existantes. Les critères de ce document restent à implémenter et prouver ; l'approbation ne constitue aucune garantie de durée. La [proposition du 29 octobre](evidence/2026-10-29/worker-registry/ai-guard-broker-supervision-design.md) reste une archive datée inchangée.

## Besoin et référence du code

**Actualisation S0,1er novembre :** la lease ne dépend plus du seul futur listener : même Arc/FD dans les owners IPC/acteurs et un propriétaire natif borné des handles. Progression Weak, arrêt sur toute sortie et vrai join avant d'attendre les STOP ; idle/NotStarted connu libère après fin des owners. Inconnu conserve l'autorité jusqu'à fin du processus. Aucun slot retiré, ledger produit ou reçu de quiescence inventé. [RED, sept fonctions PASS, revue et release natives](VALIDATION.md#1er-novembre--s0-r2-green-ciblé). Le paragraphe suivant conserve le constat initial du30octobre ; retraite complète et fencing restent obligatoires.

La mort du CLI ne doit pas abandonner l'échéance d'un worker. Aujourd'hui `runner::managed::launch_worker` et `runtime::execute_created` portent l'exécution et sa limite dans le processus appelant. Le registre, les claims irréversibles, les checkpoints privés, les verrous par tentative et la réconciliation existent ; aucun superviseur indépendant n'est livré. `in_doubt` et quota retenu protègent l'intégrité mais ne prouvent pas un arrêt.

Les contrats à conserver sont [architecture](ARCHITECTURE_CONTRACT.md), [tenants](CLOUD_TENANCY.md), [usage](USAGE_METERING.md) et [fournisseurs](PROVIDER_AUTH.md). Le code de référence est [runner.rs](../../src/runner.rs), [managed.rs](../../src/runner/managed.rs), [runtime.rs](../../src/runtime.rs) et [control.rs](../../src/control.rs).

**Correction du design antérieur :** `RunPlan` version 3 est désormais livré pour la résolution fournisseur figée. Il ne peut être renommé ou réinterprété comme un plan de supervision. Ses `provider_resolution`, `provider_input` et référence `provider_credential` sont déjà liés à l'autorisation. `credential-bindings/<UUID>.json` conserve le bootstrap sélectionné et son dictionnaire privé ; `frozen_provider_credentials` valide son digest sans recharger la source ni résoudre Auto. Ce comportement reste inchangé.

## Décisions proposées à l'orchestrateur et au mainteneur

| Décision ouverte du design initial | Choix recommandé et conséquence |
|---|---|
| D1 — présence indépendante du CLI | Ajouter `agentsandbox broker serve` au **même binaire Rust**, au premier plan, sous le compte de contrôle. Le CLI devient client local ; un petit gardien TTY du même binaire préserve le terminal après mort du client. Ce helper sans moteur/clé et son cycle de lease font partie du changement soumis, pas un nouveau microservice réseau. Générer des unités systemd/launchd déclaratives ; installation/chargement OS séparés et explicites. |
| D2 — arrêt pendant panne d'audit | Avant tout claim, rendre durable et auditer un armement qui autorise précisément l'arrêt du CID à échéance, même si le gateway disparaît. Une panne ne donne aucun droit de lancer. L'arrêt déjà autorisé ne dépend pas d'une nouvelle décision HTTP ; les observations attendent leur réconciliation durable. |
| D3 — sémantique mesurable du délai | Persister une horloge continue et une identité de boot. Publier séparément échéance, retard de déclenchement, demande stop et terminal observé. Budgets initiaux proposés ci-dessous, à qualifier ; aucune borne absolue pendant sommeil, indisponibilité moteur ou service désactivé. |
| D4 — compatibilité et migration | Ajouter une **intention immuable et un checkpoint révisionné distincts**, versionnés séparément des plans 1/2/3. L'identité ne porte jamais sur le JSON courant de `RunPlan.managed`, qui change à chaque transition. Ajouter les armements au contrôle par migration explicitement sauvegardée et protocole négocié, sans régénérer binding, claim ou durée. |
| D5 — start ambigu et fencing | Garder le claim consommé et l'arrêt ciblé actif. Une absence, un `Created` ou un retour `rm` ne suffit pas à clore. Définir puis prouver, dans chaque adaptateur, un fencing irréversible du seul CID face à l'ancien start en vol ; sans preuve, état à réconcilier et quota retenu. Pas de libération fictive. |

Ces choix ont reçu l'approbation explicite du mainteneur indiquée en tête de document ; ils n'acceptent aucun risque résiduel critique ni ne ferment un gate. L'accord sur le code n'autorise pas une installation/activation OS. La capacité D5 et la récupération d'un broker vivant figé, notamment sous launchd, restent des exigences techniques à résoudre et vérifier.

## Frontières de confiance et IPC

Le service reste dans l'Execution Plane de confiance ; gateway/dashboard conteneurisés gardent leurs rôles. Aucun socket moteur ou broker, credential runner, projet original ou privilège supplémentaire ne passe dans un worker. SQLite par tenant reste l'autorité des décisions, réservations, transitions et allocations ; le journal privé n'en est pas une seconde copie souveraine.

Une installation possède une racine canonique privée et un verrou OS détenu pendant toute la vie du broker. L'epoch du processus sert au diagnostic, jamais à réarmer un claim. Tous les profils d'une même autorité logique utilisent cette racine et ce propriétaire ; aucun verrou global ne reste tenu pendant l'attach d'un worker. Deux installations sur le même moteur exigent autorités et namespaces disjoints. Un flock local ne fence ni une racine clonée ni un autre hôte ; restaurer/copier une racine ne crée aucun droit de reprise.

Le socket Unix privé n'accepte que l'UID propriétaire. Vérifier les parents, propriétaire, mode, type et identité retenue du socket, puis les credentials effectifs du pair ([Tokio `peer_cred`](https://docs.rs/tokio/latest/tokio/net/struct.UnixStream.html#method.peer_cred), API à recouper avec la version verrouillée). L'UID n'est pas une identité tenant. Rejeter liens, substitutions, frame surdimensionnée, version inconnue, requête ambiguë et opérations hors rôle ; délais et nombre de connexions sont bornés.

Un profil de contrôle privé lie origin gateway, références runner, tenant confirmé par le gateway, autorité runtime et racines source/config/export autorisées. Chaque requête IPC exige une identité runner authentifiée correspondant au profil ; un champ `tenant_id`, chemin ou nom de profil ne la remplace pas. Viewer et worker ne peuvent demander lancement, arrêt ou export, même sous le bon UID. Aucun profil plus puissant n'est choisi pour compenser une révocation ou un refus.

L'index des credentials sert au routage vers un origin approuvé ; il ne vaut pas grant permanent. Avant accès au profil/job et nouvelle opération, le credential présenté est confirmé auprès du backend courant avec rôle, tenant et namespace attendus, puis reste lié au client RPC utilisé. L'adaptateur ne relit pas une autre clé runner pour autoriser cette requête. Révocation, rôle devenu viewer, identité changée ou backend indisponible refusent les nouvelles actions. La réconciliation autonome d'un job accepté utilise une autorité distincte, vérifiée contre son tenant/origin figés ; elle n'autorise aucun nouveau demandeur.

Le protocole ne transmet que commandes/arguments typés et chemins ou références sous ces racines. Les ouvertures réutilisent les descripteurs/validations existants ; pas de défaut « tous les fichiers du compte ». Tokens de contrôle seulement sur IPC privé, jamais dans jobs/outbox/logs. Réconciliation : relire la référence runner du profil immuablement lié au job et vérifier à nouveau tenant/origin. Révocation ou changement d'identité bloque les RPC ; l'arrêt du CID déjà armé reste autorisé.

Soumission idempotente par identité authentifiée et contenu exact : rechercher d'abord l'opération existante, avant toute lecture de source fournisseur ou résolution Auto. Un ACK « accepté » n'est envoyé qu'après durabilité du binding sélectionné, de l'intention immuable et du checkpoint initial décrits ci-dessous ; il ne signifie ni autorisation, ni démarrage, ni réussite. Perte du client après ACK ne supprime pas le job. Une revue humaine renvoie le hash exact, puis une opération d'approbation authentifiée reprend cette intention ; le daemon ne lit jamais stdin. Aucun fallback direct ou auto-démarrage du service manquant. Le gardien TTY est un helper borné d'une session autorisée, pas un mécanisme de lancement implicite du broker.

## Armement et chemin d'arrêt

1. Persister la soumission privée, puis obtenir décision/admission/quota/audit du gateway avec les contrats existants. Une erreur avant autorisation durable interdit tout lancement métier.
2. Préparer la copie via les helpers managed existants. Import, scan et export ont leurs propres identités, finalités et armements avant start ; aucun argv helper libre. L'export exige un worker terminal et une demande runner durable, vers une destination neuve.
3. Après create/inspection, conserver le CID complet et le même `Created` au contrôle. Lier un armement immuable au tenant/origin, worker/attempt, autorité/fingerprint, CID, spec/config/image, timeout admis et horloge. Aucun secret/env/corps projet dans l'armement public.
4. Persister l'armement privé puis l'audit central **avant le claim de start**. Même identité et contenu retrouvent le même reçu ; divergence = conflit. Reçu perdu : consulter/rejouer la même opération, sans recalculer l'échéance.
5. Le claim exige cet armement acquitté compatible. Relire l'échéance avant l'unique émission ; si déjà expiré, ne pas démarrer. Un claim consommé ne permet ni second start, ni restart, ni exec de remplacement. Aucun « exactly once » d'effet moteur n'est promis.
6. Un ordonnanceur dédié réserve une capacité pour inspection/stop et ne dépend ni d'HTTP/audit, ni du modèle, capture, export ou client attaché. À échéance, inspecter/arrêter **seulement le CID armé** sous l'autorité liée. Engine ID, labels, image/config divergents : ne pas adopter/tuer par nom ; état dégradé explicite.
7. Conserver l'observation terminale réelle. La transaction centrale engage terminal, allocation unique `billable=false`, quota libéré et audit. Cleanup seulement après cet ACK, sauf une future opération de fencing D5 explicitement autorisée/auditée et prouvée. Le volume et les preuves restent conservés selon leur politique.

Annuler un appel ou tuer son client Docker/Podman n'annule pas nécessairement l'effet déjà accepté par le moteur. Un start qui termine tard après un stop de `Created` reste sous enforcement ; garder le CID et le quota jusqu'à terminal crédible ou fencing prouvé. Pas de nouvelle durée après un timeout RPC. L'adaptateur D5 devra préciser l'intention auditable, la conservation de preuves et la preuve que l'ancien RPC ne peut plus agir ; sa faisabilité n'est pas présumée ici.

## Horloge, budgets et panne

Enregistrement proposé : méthode/version, plateforme, identité de boot, compteur continu entier, `armed_ns`, `deadline_ns` et UTC de diagnostic. Addition/multiplication contrôlées ; `deadline=armed+timeout` avant claim, jamais `now+timeout` lors d'une reprise. UTC ne décide pas l'expiration. Aucun fallback silencieux d'horloge.

Linux : `CLOCK_BOOTTIME`, qui compte la suspension, et `/proc/sys/kernel/random/boot_id` ([timekeeping](https://www.kernel.org/doc/html/latest/core-api/timekeeping.html), [boot_id](https://www.kernel.org/doc/html/latest/admin-guide/sysctl/kernel.html#random)). macOS : candidat `mach_continuous_time` + `kern.bootsessionuuid`, à qualifier sur l'OS/compte supportés ; convertir les ticks en nanosecondes avec `mach_timebase_info` et calcul entier vérifié, ne pas les traiter comme des nanosecondes brutes. Ne pas prendre `kern.boottime` pour une identité immuable ([header Apple](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/osfmk/mach/mach_time.h), [sysctl XNU](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/kern_sysctl.c)). Cette sélection n'est pas une qualification plateforme.

Même boot/méthode : garder la deadline exacte. Boot différent, compteur régressé/inaccessible ou méthode incompatible : aucun nouveau start ; considérer conservativement expirées les tentatives antérieures non terminales, puis inspecter/arrêter leur seul CID connu. Au réveil, traiter les échéances dépassées avant nouvelles admissions. Horloge hôte et timestamps runtime restent distincts pour l'usage.

Budgets initiaux **proposés pour la recette locale à trois workers**, pas résultats acquis : tick maximal configuré 250 ms ; déclenchement du stop au plus 1 s après échéance lorsque le service et le moteur sont disponibles ; reprise d'un broker crashé au plus 10 s sous gestionnaire actif ; observation terminale au plus 15 s après demande stop dans le cas nominal. Mesurer chaque borne séparément et refuser la qualification si elle échoue ; aucune exemption par percentile. Configurer/publier les budgets effectivement qualifiés par plateforme et profil de ressources.

Le sommeil, un moteur inaccessible ou un gestionnaire désactivé empêchent une promesse d'arrêt à temps réel. Afficher « limite atteinte, arrêt demandé », « arrêt inconnu », « terminal observé, audit en attente » ou « terminal/usage engagés », avec dernière preuve. Une panne de gateway bloque les nouveaux lancements ; elle ne suspend pas le plan d'arrêt déjà armé.

L'audit de l'armement préalable couvre déjà précisément l'arrêt à échéance. L'intention d'application et les observations sont proposées à un outbox privé borné par **enqueue non bloquant**, sans conditionner le stop à leur fsync. Un writer disque isolé effectue les writes/fsync et retourne des confirmations ; sa panne, sa file pleine ou un syscall bloqué ne sont jamais attendus par l'ordonnanceur. Aucun mutex détenu par ce writer, `PlanState::update`, HTTP ou postprocessing ne protège le snapshot nécessaire au stop : chaque armement dispose d'une cible immuable vérifiée en mémoire, avec sa propre capacité runtime réservée. Une intention non confirmée n'est pas présentée comme durable.

Les nouvelles admissions/armements exigent leur vraie durabilité et restent refusés tant que le writer ne progresse pas. Pour les armements déjà engagés, l'ordonnanceur tente le stop indépendamment du writer et conserve au plus un état de résultat borné par job en mémoire ; aucune capture illimitée. Après retour du stockage, réconcilier les identités exactes sans double opération. Si la mémoire de résultat disparaît pendant la panne, retrouver les observations sur le même CID conservé ; ne pas affirmer qu'une preuve locale non écrite a survécu. Pas de cleanup, faux ACK, prix, allocation zéro ou quota libéré pour compenser cette perte.

Disque plein/readonly ou fsync suspendu : un armement durable et sa cible encore lisible ou vérifiée en mémoire permettent l'arrêt déjà autorisé, en exposant le défaut de journal. Après restart, si preuve du CID et gateway sont tous deux indisponibles : récupération contrôlée requise, aucun kill large. La séparation empêche la contention du journal de bloquer le code de stop ; elle ne rend pas accessible un moteur ou un exécutable lui-même bloqué par l'OS.

La pression bytes/inodes et la capacité réservée aux preuves/arrêts doivent être qualifiées ; ne pas effacer des preuves pour restaurer le service. Une instance arrêtée sans timestamps fiables garde la mesure inconnue. Une clôture avec quota libéré mais usage indisponible nécessite un contrat séparé, explicitement audité et prouvé ; ce design ne l'introduit pas implicitement.

## Intention immuable, checkpoints et secrets

Le JSON `RunPlan` actuel est un checkpoint : `PlanState::update` y remplace notamment `managed.worker`, `pending`, observations et résultats. **Ne pas hasher ce fichier mutable comme identité permanente.** Proposition `ExecutionIntentV1` : snapshot privé typé des champs stables `plan_version`, `operation_id`, `session`, options initiales, demande initiale, `provider_input` et référence de binding figé ; exclure `managed` et le descripteur transitoire `credential_source`. Son hash propre utilise un domaine `ai-guard.execution-intent.v1`, des octets JSON canoniques définis/versionnés (clés ordonnées, ordre des tableaux conservé, aucune normalisation textuelle implicite). Il ne remplace ni ne modifie les hashes existants des décisions, admissions ou fournisseurs.

Avant résolution, une réservation privée du job lie tenant/origin authentifiés, installation, identité d'opération et hash du payload `SubmitV1` exact, sans token de contrôle ni numéro de FD. Sous le verrou de ce job, un rejeu compare ce payload à la réservation et charge l'intention déjà figée ; il ne choisit jamais de nouveau client/credential. Pour une nouvelle opération, synchroniser binding fournisseur puis intention et checkpoint0 ; écrire ensuite un marqueur d'acceptation et synchroniser son parent avant ACK. Une préparation interrompue sans intention/binding complet reste « préparation interrompue, nouvelle opération requise », sans ACK accepté ni lancement. Des fichiers partiels conservés ne sont pas un droit de refaire Auto sous la même identité.

Proposition `ExecutionCheckpointV1` : `intent_hash`, révision monotone, référence de révision précédente et état mutable managed/armement/outbox. Le writer est unique sous verrou d'autorité/tentative ; il écrit une nouvelle révision privée, la synchronise, puis publie atomiquement le pointeur de révision et synchronise le parent avant de confirmer cette transition. Au restart, charger la dernière révision confirmée et vérifier son intention ; recouper systématiquement claims et opérations avec le contrôle avant tout effet. Un ancien checkpoint ne redonne aucun claim. Approbation et adoption d'une invocation runtime sont des opérations explicitement autorisées/révisionnées liées à l'intention, pas des réécritures silencieuses de son snapshot initial.

Les plans 1/2/3 existants restent lisibles sous leur format ; les hashes des demandes/admissions et bindings qu'ils portent restent inchangés. Le nouveau broker n'ajoute pas de champs à leurs JSON stricts. Une migration quiescente peut créer les nouveaux enregistrements à partir d'un snapshot privé vérifié, sans écraser l'original ni permettre un ancien writer concurrent. Pour un plan 3, le broker rouvre **le binding privé figé**, valide digest et cohérence de résolution/argv, puis prépare le handoff minimal existant. Source `.env`, profil Auto et abonnement/API ne sont pas résolus à nouveau après ACK/approbation/restart. Binding manquant/modifié avant premier start : refus, jamais remplacement par une source tournée. Le stop d'un worker déjà parti n'exige aucun credential fournisseur.

Les plans 1/2 conservent leur sémantique et références existantes. Pas de conversion, réimport, adoption d'autorité ou libération automatique. Avant migration : inventaire quiescent et sauvegarde vérifiée des bases, plans, bindings et journaux, sans copie publique de secrets. Une tentative non consommée peut recevoir son premier armement après validation complète ; un claim déjà consommé ne redonne jamais droit de start. Pour les anciens actifs sans horloge persistable, arrêt conservateur du CID lié et réconciliation, pas de nouvelle fenêtre complète.

Le protocole API/armement doit être explicitement versionné et négocié avant migration de schéma ; ne pas ajouter de champs silencieux à des types `deny_unknown_fields`. Un ancien contrôle sans capacité d'armement ne peut démarrer via le nouveau service ; lecture/arrêt/export compatibles restent à définir/tester. Upgrade drain des admissions tout en continuant l'enforcement ; rollback ou désinstallation avec actifs non terminaux refusés par défaut. Restaurer un backup ne réarme ni claim ni deadline.

## Propriétaire du terminal et mort du client

Le `Drop` du CLI ne s'exécute pas après SIGKILL. Proposition : le CLI lance explicitement un **gardien TTY Rust du même binaire**, sans moteur, token fournisseur ou rôle runner général, qui tient une copie du FD du terminal physique et son `tcgetattr` initial **avant toute entrée raw**. Il est détaché de la durée de vie/groupe de signaux du client et n'appartient pas au job/cgroup du service broker ; sa survie lors de ces morts doit être testée, pas déduite d'un fork. Le broker reste propriétaire du runtime/PTY et des échéances. Le gardien ne lance aucune commande métier ni service OS.

L'IPC UID+runner autorise un grant d'attach limité à une tentative, un terminal, un epoch et une génération de lease, sans faire du gardien une autorité tenant. Transférer les FD par canal Unix privé, jamais par pathname fourni librement ([SCM_RIGHTS Linux](https://man7.org/linux/man-pages/man7/unix.7.html), [identité du pair Darwin](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/sys/un.h)). Vérifier FD caractère/isatty, identité dev/inode/rdev, boot, session terminal et groupe foreground du **client identifié par le noyau**, pas ceux du daemon et pas un PID JSON non prouvé. L'identité du processus doit aussi exclure une réutilisation de PID par une preuve native de naissance/identifiant unique ; UID et PID seuls ne suffisent pas. Si ces preuves ne peuvent être validées sur l'OS supporté, refuser l'attach avant raw. Après exec, environnement reconstruit sans credentials et liste fermée de FD (terminal, liaisons lease et sortie filtrée) ; cloexec sur les autres FD, aucun FD surprise/tronqué, jamais de fuite de FD vers worker/fournisseur. Employer un FD terminal écriture/attributs sans accès clavier lorsque l'OS le permet et qualifier ce mode avant raw.

Le gardien ne lit **aucune frappe** et ne conserve aucun transcript : le CLI lit le clavier et envoie seulement des frames de la génération autorisée au broker. Le gardien est le seul émetteur physique des sorties déjà filtrées pendant la lease ; stdout/stderr du CLI ne contournent pas ce canal. Il enregistre lui-même les attributs effectifs, partage une copie FD/état initial avec le broker **et le CLI** pour secours, puis acquitte `prepared`. Avant raw et premier claim, exiger lease autorisée, liaisons client/broker vivantes, état initial conservé et validation du terminal ; le gardien entre raw et acquitte `active`. Une liaison privée dédiée à chaque propriétaire, sans extrémité fuitée, détecte fermeture/EOF ; heartbeat et contrôle de génération bornés traitent une liaison figée. Un seul gardien possède la lease de ce terminal dans l'installation.

Sortie normale, EOF client, mort du broker ou expiration de lease : **révoquer d'abord la génération clavier/sortie et abandonner le backlog**, le gardien cessant immédiatement d'écrire les frames de cette génération. Le CLI survivant arrête ses reads/frames ; le broker rejette toute frappe retardée ou rejouée. Ensuite reset de présentation fixe borné et `tcsetattr(TCSANOW)` depuis le FD retenu ; relire les attributs effectifs avant ACK `restored`, fermeture des FD et restitution coopérative du terminal. Ne pas attendre drain infini, ACK runtime/HTTP ou fsync pour restaurer. Le broker/CLI survivant possède le secours FD+état initial si le gardien meurt ; aucun succès ne repose seulement sur Drop. Si tous les détenteurs du FD meurent ou si le terminal disparaît, ne pas rouvrir un nom de PTY potentiellement réutilisé ; récupération opérateur explicite, jamais « restauré » inventé.

Avant toute restauration, vérifier l'identité/session retenues, la génération de lease et le foreground actuel : groupe du client, ou groupe appelant connu ayant récupéré le terminal après sa mort, avec attributs toujours ceux installés par la lease (ou déjà ceux d'origine). Un nouveau consommateur/groupe inconnu, une modification concurrente ou un FD devenu invalide interdit de lui imposer d'anciens modes ; afficher perte de propriété/restauration incomplète et garder un diagnostic sans frappe. Ces contrôles ne prétendent pas être un verrou atomique contre un administrateur ou autre processus du même UID modifiant simultanément ce terminal. Les scénarios de changement de foreground sont des critères négatifs, pas des restaurations forcées.

Canal/frames/resize bornés, sans transcript ni log de frappes/écrans ; compteurs inconnus conservés. Mort du client ne bloque pas l'ordonnanceur, ne rejoue pas les frappes et ne donne aucun nouveau start. Le réattachement après claim consommé n'est pas livré et ne devient pas acquis par ce design. Le vrai parcours CLI/TUI et sa restauration après SIGKILL sont obligatoires ; pas de repli batch.

## Installation et visibilité

Les noms `broker serve`, `broker service plan/install/status`, `broker doctor`, `broker drain/reconcile` sont des **interfaces proposées**, pas des commandes existantes. `plan` rend fichiers/diff/compte/racine concrets ; `install` est une action opérateur distincte. Aucun sudo, changement de linger, unité préexistante remplacée ou job launchd chargé par ce document.

[systemd](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html) peut superviser/redémarrer un service ; son timeout de processus n'est pas une limite appliquée à un conteneur externe. [launchd](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html) distingue daemon et agent de session : un LaunchAgent ne qualifie pas une présence après logout. KeepAlive ne prouve pas la récupération d'un processus vivant suspendu/bloqué ; il faut définir/qualifier le mécanisme de détection/récupération correspondant, ou garder ce cas FAIL. Aucune capacité de watchdog macOS n'est supposée déjà livrée.

Doctor distingue unité générée/installée/chargée, processus et progrès d'ordonnanceur, identité boot/clock, autorité moteur disponible, compatibilité de protocole, jobs/outbox et seuils dépassés. Un heartbeat périmé n'autorise ni takeover ni libération. Un statut de supervision typé publié par le broker au gateway reste `declared`, filtré par tenant ; il ne remplace pas un contrôle OS indépendant. Le dashboard unifié ne monte aucun socket/credential supplémentaire. Ni cloud distant, ni HA, ni PKI externe, ni Kubernetes n'est installé par ce chantier.

## Acceptation et séquence après décision

| Preuve exigée | Critère de recette et erreurs à conserver |
|---|---|
| Client mort | A1/A2 chez A, B1 chez B ; SIGKILL des clients, échéances conservées et arrêts ciblés réels, quotas/usage isolés. Un témoin non expiré n'est pas arrêté. |
| Broker mort/repris | Tuer le service avant/après armement, claim et stop ; reprise par vrai gestionnaire, même deadline/CID, aucun second start, une allocation finale. Tester aussi gel/SIGSTOP, crashloop et gestionnaire désactivé ; absence d'enforcement reste FAIL, pas scénario ignoré. |
| Gateway/audit/disque en panne | Armement durable préalable, stop à échéance sans nouvel allow, outbox/reprise exacte, quota retenu jusqu'au terminal engagé. Writer/fsync bloqué et mutex de checkpoint occupé : le stop préarmé reste déclenché. Pannes disque/saturation sur données de test ; aucune preuve existante effacée. |
| Start encore en vol | Faute contrôlée sur vrais Docker/Podman : ACK perdu, démarrage tardif, Created/absence trompeurs, fencing D5 et zéro doublon. Aucune libération fondée sur timeout/absence seule. |
| Horloge | UTC simulé avant/arrière, même boot, boot différent, overflow/régression/erreur ; puis sleep/wake et reboot sur environnement autorisé. Pas de changement d'heure/reboot du poste partagé sans accord approprié. |
| Identité/IPC | Pair/runner faux, viewer/worker, A→B par corps/path, socket/parent substitué, frame grosse, rejeu divergent, autorité/CID/config remplacés : aucun effet non autorisé. |
| Plans et secrets | Checkpoint évoluant sans changement du hash d'intention ; rejeu après perte d'ACK, crash entre chaque fsync/marqueur, révision ancienne/refus de writer concurrent. Reprises 1/2/3, source tournée mais binding3 identique, binding altéré/manquant, approbation exacte, zéro re-résolution/fallback ; canaris masqués. Arrêt sans clé fournisseur. |
| CLI et TTY | Vrai PTY physique : SIGKILL du PID client pendant raw, EOF/lease, broker tué, gardien tué et secours survivant ; vérifier attributs effectifs, arrêt des frames avant restitution et absence de replay. Témoin d'un autre terminal inchangé, nouveau foreground inconnu jamais réécrit. Install/doctor/revue/submit/status/stop/export et absence de capture secrète restent obligatoires. |
| Upgrade/restauration | Unités préexistantes préservées, drain/enforcement, lecture N/N-1, rejet d'ancien protocole, backup/restore avec fencing et aucun réarmement. |

Ordre : décision structurelle et relecture de constructibilité → contrat/RED pertinents → horloge/journal/armement → service/IPC → intégration CLI/TTY/OS → fautes réelles A1/A2/B1 → revue et qualification. Tous les points restent obligatoires, sans étiquette MVP. Les tests sont confiés à Luna, ciblés selon le risque, sans modèle fournisseur ni campagne E2E systématique. Docker/Podman et Linux/macOS gardent des preuves distinctes ; la charge 100 est séparée et n'est pas imposée à la recette standard.

**État courant :** direction approuvée et relue ; contrats, Store3, API v1, producteur d'horloge et format d'intention ont leurs preuves distinctes dans [CURRENT_STATE.md](CURRENT_STATE.md). Le [journal privé](evidence/2026-10-30/broker-journal/README.md), le [bootstrap exclusif de contrôle3 neuf](evidence/2026-10-30/supervision-bootstrap/README.md), le [checkpoint de supervision V2](evidence/2026-10-30/broker-checkpoint-v2/README.md) et le [cycle de vie local V3](evidence/2026-10-31/broker-lifecycle/README.md) ont leurs vérifications logicielles nommées. La preuve Docker du bootstrap concerne le contrôle, sans worker ; les faits runtime des tests V2/V3 sont synthétiques. Raccord service/IPC, ordonnanceur, gardien et D5 restent à livrer. L'installation/charge d'unités OS n'est pas autorisée par l'accord de code. Le FAIL réel du crash CLI demeure ouvert ; les composants préparatoires ne qualifient pas la supervision autonome. Aucun gate fermé par cette direction.

### Raccord concret au service — obligations toujours ouvertes

Le checkpoint V2 représente les requêtes/réponses réelles du contrôle. Le lot V3 du 31 octobre ajoute l'annulation avant admission, la demande d'approbation séparée et les échecs locaux typés ; le journal permet aussi le rejet avant intention valide. Ces formats ne constituent pas encore le moteur du service. Leur raccord doit conserver ces faits durables, sans faux Worker/refus policy/receipt ni fichier parallèle de statut. Une annulation acquittée ne peut disparaître au redémarrage puis laisser le job repartir.

L'IPC ne reçoit pas les paramètres de contrôle libres de RunOptions : origine du gateway, credentials, racines privées et invocation moteur viennent du profil de confiance lié à l'identité fraîchement authentifiée. La même credential présentée sert aux actions ; aucun secours par lecture de la clé d'un autre profil. Authentifier avant d'ouvrir projet, fournisseur ou job. Sur Submit rejoué, retrouver le binding durable avant toute nouvelle résolution Auto ou lecture d'une source ayant tourné. Le service impose l'UUID de Submit à la préparation ; `choose_plan` ne doit pas le remplacer silencieusement.

Le premier batch permanent doit raccorder les vrais helpers Import/ImportVerification puis worker, FinalScan et export explicite aux armements/claims. Le `Ctrl-C` global du runtime, son timer local et le monitor qui réécrit le plan à chaque poll ne peuvent être repris tels quels dans un daemon partagé. L'échéance préarmée utilise sa cible immuable et un chemin d'arrêt séparé de l'IPC, du writer/fsync, de HTTP et de la clé fournisseur. Une réponse EmitOnce restaurée reste ObserveOnly ; une nouvelle tentative ou un restart ne recrée ni claim ni durée. La qualification d'un batch après mort CLI ne couvre pas le gardien TTY, le fencing D5 ou les autres plateformes.

### Raccord en cours — interfaces d'implémentation du 31 octobre

Base `c4c4d2a7560d31768da08d8ad9335f4f08667344`. Cette section fixe les coutures du service en cours, **pas des commandes livrées ni une qualification**. Le vrai CLI ne reconnaît pas encore `broker` au RED conservé. Le transport Unix a son propre RED puis trois tests ciblés sur sockets réels ; une révision de ses oracles suit la revue indépendante. Autre UID et Linux ne sont pas qualifiés par ces essais macOS.

**Décision d'implémentation dans la direction approuvée :** un sous-processus interne du même binaire porte les seuls arrêts préarmés, avec une lease distincte du writer broker. Ce composant reste dans le contrôle de confiance local, sans nouveau service réseau, microservice ni installation OS. Le reviewer de constructibilité a confirmé cette frontière. Il ne reçoit ni token runner, credential fournisseur, projet, commande métier ni droit start/exec/remove ; le profil et l'identité tenant ont déjà été recoupés par le broker. UID noyau et capability privée protègent son lien interne ; ils ne sont pas une identité tenant.

Le transport commun `broker::local_ipc` fournit `PrivateListener::bind_new`, un `EndpointRecordV1` opaque strict et `connect_checked`, `accept_checked`, `read_frame` et `write_frame`. Chaque listener crée un sous-répertoire privé neuf et un socket0600 ; son record lie epoch, identité du répertoire et du socket, sous racine0700 retenue. Aucune entrée préexistante n'est supprimée ou adoptée. Les annonces d'endpoint restent des données de découverte, jamais une autorisation ; les découvrir ne suffit pas à lire un job. Frames JSON fermées avec longueur BE32, cap512KiB maximum et timeout total préfixe+corps. La limite128générations est logique, sans quota physique bytes/inodes qualifié. Les appels du produit doivent encore appliquer leurs bornes plus petites, leurs leases, authentification et backpressure.

Root possède les profils privés, découverte/leases, contexte fraîchement authentifié, IPC/CLI et `broker::writer::WriterClient`. Tous les accès Journal, même load/status/list, passent par un thread writer unique et une file bornée ; aucun RPC, copie/archive ou callback métier n'est exécuté dans ce thread. Méthodes utilisées par le batch : `load(operation_id)`, `advance_lifecycle(operation_id,expected,next)` et `append_command(operation_id,command_id,request)`. Le writer construit les hashes dans le vrai contexte du job et ne confirme qu'après le reçu durable. Les commandes IPC restent authentifiées avant profil/job/projet de la requête ; les paramètres de contrôle sont imposés par le profil. Une erreur, file pleine ou réponse perdue ne vaut jamais mutation refusée certaine ni ACK factice.

Le port interne `LastConfirmedStatus` du candidat du9novembre est une projection
mémoire de diagnostic, sans accès Journal hors de cet acteur. Status/RelayStatus
conservent l'authentification courante et les wire V1 fermés ; ils peuvent lire
la dernière projection issue du chemin strict et publiée avec son ACK acteur.
Avant cet ACK, la tête précédente reste visible. Cache froid/hors capacité :
lecture stricte, jamais absence inventée. Bornes : jobs du journal et au plus256,
bytes du périmètre et au plus4Mio de données de projection comptées. Ce cache
ne conserve ni AcceptedJob, bindings, historique ou permission ; décisions et
mutations continuent leurs Full pré/post, CAS et audit. Échec strict d'intégrité,
publication inconnue, fencing/arrêt ou perte de l'owner ferment le port jusqu'à
nouveau propriétaire après contrôles stricts de reprise. Une couverture STOP
partielle due à une Arm légitimement pending n'est pas une erreur d'intégrité :
le statut confirmé conserve son attente explicite. Aucun changement du cache ledger Parsed/Tainted
ou de PrivateFile ne découle de ce diagnostic. Les oracles d'acteur, de durabilité
et le parcours Full3 doivent passer avant d'attribuer un résultat au candidat ;
le FAIL F1 et la qualification D5 restent ouverts.

La capacité `ProfileWriterCapabilities` retient une seule admission d'acteur par rapport de reprise et partage sa lease existante sans déverrouillage. `full()` est absent lorsque le profil n'admet que les arrêts. `stops()` fournit uniquement `append_cancel(CancelInputV1)`, `cancel_stop_projection` et `recovery_status`, sans conversion vers le chargement métier ou une permission de start. Le handler Cancel n'exige plus le blob fournisseur ni le writer complet ; il invalide d'abord la génération pour une nouvelle commande, publie durablement puis livre les cibles prouvées. Un retry inconnu conserve son identité et n'invalide pas à nouveau ; un replay confirmé ne crée aucune révision. Les budgets incomptables refusent une nouvelle mutation, sans retirer une annulation confirmée. Le reader métier garde ses vérifications strictes avant/après publication. Le RPC/CLI additif de diagnostic est documenté dans le [quick start](../quickstart.md) ; sa provenance journal n'est pas une observation moteur. [Décision et validation](DECISIONS.md#31-octobre--acteur-stop-restreint-et-diagnostic-de-reprise).

Le core possède `runner::broker::executor::BatchExecutor`, ses builders privés dans runner et les seules coutures nécessaires de `runner/managed.rs`. `run(operation_id,context)` et `export(operation_id,target,context)` sont distincts. Le contexte non sérialisable lie le même AuditClient/credential présenté, scope réel, config retenue, runtime approuvé et génération de cancel ; l'export cible une destination neuve sous racine permise. Aucun wrapper de `execute_plan` legacy, `sessions.json`, monitor500ms, timer CLI ou argv helper libre. Le moteur suit Decision/Review, copie/archive, admission supervisée, Prepare, Import, ImportVerification, CreateIssued, worker, terminal central, FinalScan puis export explicitement demandé. Chaque requête naît pending avant son RPC, son ACK dans une révision suivante. Après conflit CAS, fusionner seulement la réponse du pending exact en préservant les commandes courantes ; ne pas refaire une action pour résoudre le conflit.

`broker::enforcer::EnforcerClient` reconnecte d'abord le sous-processus existant ou le lance explicitement depuis `broker serve`, jamais depuis un client qui découvre un service absent. `register` lie ArmV1/reçu, scope de contrôle, RuntimeAuthority **et invocation figée**, OwnedContainerSnapshot et deadline native originale. Replay exact conserve l'état et l'échéance ; divergence est conflit. L'ACK exige autorité utilisable, stop actor et capacité effectivement réservés avant claim, pas seulement une entrée de map. Préparation/handshake/IO lourds ne monopolisent pas le dispatcher de stop. Chaque cible a sa capacité réservée ; le cap borne aussi les threads, canaux et mémoire, sans prétendre qualifier100workers.

Une permission de start fraîche est un objet opaque non Clone/non Serde construit seulement par le batch depuis le vrai résultat HTTP EmitOnce recoupé puis ACK local durable. Il lie arm/claim/emitter/sujet/worker/attempt/namespace et génération. `EnforcedSlot::prepare_start` le consomme ; le vrai `BeforeSpawnGuard` vérifie encore epoch/slot/progression, génération mémoire monotone et horloge native avant spawn. Aucune disposition chargée du journal ne recrée cet objet. L'invalidation mémoire de cancel précède l'attente du writer ; l'arrêt anticipé demandé au child exige ensuite la commande durable, tandis que l'échéance préarmée continue sans nouvel audit. Un bref verrou mémoire ordonne cancel/garde, sans promettre l'atomicité de l'effet moteur face à un start déjà en vol.

Le child continue après SIGKILL/SIGSTOP du seul broker et accepte une reconnexion authentifiée. Son lock n'est pas le lock broker et aucun FD de ce dernier n'est hérité. Socket muet/timeout ou lock occupé ne donnent aucun unlink/takeover. Les cibles déjà connues restent armées jusqu'à résolution ; panne du child ferme les nouveaux starts et la reprise n'utilise que les mêmes cibles et deadlines. La course santé/spawn n'est pas atomique en espace utilisateur et exige une preuve dédiée ; la panne conjointe dépend toujours du gestionnaire OS non installé/qualifié ici.

**Désarmement : condition non présumée.** Un `ManagedPrearmedExecution` Ok, la fin du client ou un code de sortie seul ne prouvent pas que le start RPC n'est plus en vol. Il faut une provenance vivante de complétion normale de l'emitter/claim exact, le terminal admissible du même snapshot et son ACK central vérifié ; un nonzero n'est pas universellement une erreur transport. Sans témoin établi par l'adaptateur et ses tests, ne pas fournir de constructeur de preuve ni désarmer. Perte de preuve au restart, Created, absence ou timeout restent inconnus avec slot/ressources retenus et D5 ouvert. Aucun faux booléen persistant n'est un substitut.

Deux erreurs de préparation restent à traiter explicitement : archive créée avant managed durable (reprise bornée depuis l'archive seule figée ou NeedsReconciliation, jamais nouvelle copie silencieuse de la source), et export dont l'original n'est plus identifiable par `extract_workspace`. Elles ne disparaissent pas sous l'étiquette batch livré. Préparer/cancel sans CID ni terminal admissible peut encore retenir quota ; aucun faux Cancelled ou usage0.

## Contrat de retrait du gardien — conception à implémenter, 1er novembre

Cette section consolide le contrat D5 V2, ses correctifs V3 et la revue V3 ; elle ne change pas l'architecture approuvée.
Les notes privées sources sont `ai-guard-d5-retirement-contract-v2.md`, `ai-guard-d5-contract-v2-review.md`,
`ai-guard-d5-retirement-contract-v3.md`, `ai-guard-d5-contract-v3-review.md` et `ai-guard-d5-ledger-next-vertical.md`.
Leurs décisions utiles sont portées ci-dessous ; ces fichiers locaux ne sont pas requis pour lire le contrat distribué.
Les sections et résultats datés précédents conservent leur périmètre historique.

### État du code et portée

StopRecovery apporte un transport RecoveryV2 explicite, authentifié avec HMAC-SHA256 et rôle demandé lié au challenge,
des cibles STOP issues de rapports/projections propriétaires, leurs identités et échéances originales, et des tentatives bornées.
V1 et son `EnforcerClient` Full legacy restent distincts. StopsOnly ne se convertit pas en Full ; FullV2 est indisponible.
Le ledger est toujours `Unverified` : aucun sceau/floor/delta/tombstone, Closing, reçu de quiescence, Pending/Final V5
ou port de finalisation de retrait n'est livré. Une lease O_RDWR et un thread démarré ne constituent pas ces preuves.
Dans R8, LedgerState n'admet que Unverified et ChallengeV2 est fermé (`deny_unknown_fields`). Ajouter Verified/coverage/Full
ne garantit donc aucune compatibilité avec ces clients : stratégie de versionnement/mise à jour explicite et testée requise avant raccord.
La direction retenue est la négociation V3 explicite avant tout accord, décrite ci-dessous. Le candidat initial R4b a quatre écarts P2, reproduits après correction mécanique de compilation R4c. Le candidat1 les corrige ; revue source et32 fonctions ciblées PASS, puis fmt/Clippy strict/release du checkpoint composé D5/TLS passent. Le lot est publié sous a334b36, composé avec TLS766b512, sans preuve du ledger ou de migration automatique des appelants2 vers3. Les résultats historiques de chaque gel restent distincts.
Préserver refus fermé et STOP connus, sans conversion d'anciennes connexions ; le contrat complet3 et sa règle de forme embarquée sont précisés plus bas.
R7 passe dix fonctions ciblées, dont les deux reprises multi-Arm après abandon du waiter, puis échoue à Clippy strict.
R8 corrige seulement une indirection privée et deux conditions équivalentes ; fmt, Clippy strict, quatre canaris complémentaires,
deux tests CLI et une compilation release passent. Les sources et trois exécutables release exacts sont conservés.
Les [résultats par gel](VALIDATION.md#r7--correctif-groupé-vérifié-clippy-refuse-trois-diagnostics) font autorité sur les preuves,
avec [l'état constaté](CURRENT_STATE.md), les [décisions](DECISIONS.md#1er-novembre--reprise-stop-distincte-de-ladmission)
et les [gates](PRODUCTION_READINESS.md). **G09 reste FAIL, D5 ouvert ; aucun gate ni effet moteur n'est qualifié ici.**

### Négociation V3 avant tout accord — contrat du premier test

Choix technique de Root : conserver la préface et les échanges V2, puis permettre une négociation authentifiée V3
avant tout Grant. Une mise à jour appariée annonçant directement V3 serait plus simple, mais ne fournirait pas la continuité
du vrai client2 vers le serveur étendu. Aucun deuxième listener, service ou changement de root n'est décidé.
Challenge/Auth/Grant/Status/Watch2 gardent leurs schémas, domaines et valeurs actuels ; les nonces restent frais.
La vue2 conserve StopsOnly, Unverified(recovery_transport_only) et admission_available=false, sans champ coverage3 ni Full implicite.
Auth2 Full reste refusé ; une connexion déjà accordée ne renégocie jamais sa version ou son rôle.

Après lecture et validation de la vraie préface2, sur une connexion non encore accordée, le nouveau message cible est fermé :
`{kind:"negotiate_v3", protocol:3, requested_role:"stops_only"|"full", client_nonce:UUID, proof:Hash64}`.
Le nonce client est canonique, frais, non nul et distinct du nonce serveur2. La preuve est HMAC-SHA256 avec la capability IPC existante,
sur `b"ai-guard:enforcer-client-negotiate:v3\0"` suivi de la sérialisation JSON du tuple exact :
`(challenge2.protocol, challenge2.role, challenge2.epoch, challenge2.nonce, challenge2.root_binding, challenge2.offered_roles, challenge2.ledger_state, 3_u32, requested_role, client_nonce)`.
Les sept premières valeurs viennent de la préface effectivement reçue et recoupée, sans valeurs par défaut ni clé nouvelle.
proof est du hex minuscule sur 64 caractères ; la clé n'est ni changée ni exposée.

La sélection de première authentification distinguera directement les deux schémas fermés Auth2/Negotiate3 sur les bytes bornés.
Refuser doublons, formes mixtes, champs inconnus, mauvais types, trailing JSON et nonce/proof non canoniques ;
ne pas classifier via Value→from_value ou un repli permissif perdant des clés. Le cap handshake reste 4096 bytes.
V3 aura une échéance monotone globale de 6 s depuis l'entrée du handshake et les timeouts de trame de 3 s,
avec au plus deux allers-retours après la préface2 ; la voie2 garde ses sémantiques actuelles.
Un refus3 ferme cette connexion, sans Auth2 compensatoire ; un besoin explicite STOP2 emploie une autre connexion,
sans satisfaire une demande Full3 par une capacité STOP.

Le premier test demande stops_only et attend une trame distincte kind=challenge_v3, protocol=3, role=enforcer,
epoch et root_binding inchangés, client_nonce corrélé et champ unique server_nonce frais, non nul et distinct des deux nonces précédents.
Aucun alias nonce n'est admis. Cette trame seule ne constitue ni Grant3, ni ledger vérifié, ni effet STOP ou pouvoir Full.
La suite Auth3/Grant3 et Status/Watch3 suit le contrat ci-dessous ; sa présence dans un candidat ne prouve pas sa validation.
Full3 exigera Complete ; Partial/Unverified refusera Full sans Grant STOP implicite.

Le brouillon qui attendait coverage3 dans Status2 n'a jamais été exécuté et est corrigé avant Cargo.
Le premier RED R9 garde les positifs Journal/Cancel/Restore V2 et atteint l'oracle de négociation3 absente sur une autre connexion,
après correction du cleanup du harnais avant exécution. Diagnostics Partial et handshake3 complet restent NOT_RUN ;
aucun simple message ne clôture la verticale D5.

### Transport STOP3 complet — contrat du candidat R4b, non qualifié

Le contrat ci-dessous est décidé avant les corrections et tests du candidat. Le gel privé R4b conserve les14 tests initiaux et ajoute quatre régressions de revue ; il préserve R9. R4, non exécuté, reste archivé : son positif de présence null contredisait les cinq omissions historiques ; aucune exécution ni RED ne lui est attribué.

Les records propres à3 sont fermés, sans défaut implicite, avec chaque Option explicitement null ou valeur. UUID :36 caractères canoniques minuscules et non nul ; digests/proofs :64 caractères hex minuscules. Les données V1 embarquées suivent **exactement la forme de leur sérialiseur historique** : tout membre émis est requis, null compris ; tout membre omis par `skip_serializing_if` reste absent lorsque None. Les cinq exceptions sont `RuntimeAuthority.invocation`, `RuntimeAuthority.docker_launch_profile`, `InvocationBindingV1.executable_directory`, `ContainerHandle.configuration_fingerprint_version` et `ContainerObservation.configuration_fingerprint_version`. Cette règle ne modifie pas leurs sérialiseurs ni le décodeur2.

Une visite récursive bornée refuse chaque clé JSON décodée dupliquée avant insertion dans une map, y compris dans une branche inconnue. Le décodage typé part des octets originaux, jamais d'une Value qui aurait normalisé nombres ou doublons. Le témoin structurel est comparé à `Serialize(T)` : mêmes membres, indices/longueurs de tableaux, positions null et catégories scalaires ; ordre des clés sans autorité. Les validateurs typés et MAC restent obligatoires. Retirer un null émis, injecter un null omis, mélanger les formes ou ajouter une valeur JSON après la trame ferme le canal avant dispatch/publication. Une capture `RawValue` est autorisée si nécessaire, sans nouveau paquet/version ; elle ne choisit ni rôle ni protocole.

Pour les preuves suivantes, N désigne le tuple Negotiate3 déjà défini, C/A/G/F les tuples des cores ci-dessous dans l'ordre écrit, sans leur proof. `J` est le helper canonique du journal appliqué à ces valeurs typées : clés triées récursivement, tableaux conservés. Le transcript N historique conserve **`serde_json::to_vec(N)`**, distinct de J. Tous les préfixes de domaines se terminent par un NUL. `sid=SHA256(b"ai-guard:enforcer-session:v3\0" || J((N,C)))` ; même capability IPC, HMAC-SHA256 vérifiée sans comparaison textuelle précoce.

| Record3 | Core dans l'ordre ; chaque message porte aussi proof |
|---|---|
| Challenge | `kind=challenge_v3, protocol=3, role=enforcer, epoch, root_binding, requested_role, client_nonce, server_nonce, offered_roles, ledger_state` ; le message ajoute le champ `session_id=sid` avant proof |
| Auth | `kind=auth_v3, protocol=3, session_id, requested_role, epoch, root_binding, client_nonce, server_nonce` |
| Grant | `kind=granted_v3, protocol=3, session_id, granted_role, epoch, root_binding, client_nonce, server_nonce, ledger_state, admission_available` ; ce lot impose `granted_role=stops_only`, `admission_available=false`, ledger Unverified |
| Refusal | `kind=refused_v3, protocol=3, stage=negotiate\|authenticate, requested_role, epoch, root_binding, client_nonce, server_nonce, session_id, code, ledger_state` |

| Preuve3 | Domaine après le préfixe commun `ai-guard:enforcer-`, puis projection |
|---|---|
| Challenge | `server-challenge:v3\0` + `J((N,C,sid))` |
| Auth | `client-auth:v3\0` + `J((N,C,sid,A))` |
| Grant | `server-grant:v3\0` + `J((N,C,sid,A,G))` |
| Refusal | `server-refusal:v3\0` + `J((N,Option<C>,Option<A>,F))` |

Le client vérifie Challenge avant Auth, puis Grant et toutes les corrélations. Epoch/root correspondent à la préface et aux ancres retenues ; les trois nonces sont frais, non nuls et distincts. La préface2 est contrôlée lexicalement par le seul client3 avant N3, sans durcir rétroactivement le vrai client2. Aucun alias nonce, réemploi de session, mélange de domaine ou changement de rôle après accord.

Full3 valide est refusé immédiatement après N3 authentifié, avec code `full_unavailable`, C/A/sid null et nouveau nonce serveur ; pas de Challenge ni Grant STOP compensatoire. Les autres codes fermés sont `ledger_changed`, `transport_unavailable`, `protocol_unavailable`. MAC invalide, timeout ou ancre invérifiable ferment sans prétendre produire un refus authentifié. Après Auth valide, un refus corrèle C/A/sid et le nonce de C. Aucun fallback2 sur cette socket ni client Full3 fictif.

Le contexte de session est privé et issu du handshake vérifié. Une seule commande par connexion, sauf Watch sortant continu. `Request3Core=(kind=request_v3,protocol=3,session_id,epoch,request_id,command)` ; `Reply3Core=(kind=reply_v3,protocol=3,session_id,epoch,request_id,sequence=0,result)` ; `Watch3Core=(kind=watch_v3,protocol=3,session_id,epoch,request_id,sequence,value)` ; chacun ajoute proof. Domaines : `client-command:v3\0`+`J((sid,RequestCore))`, `server-reply:v3\0`+`J((sid,RequestCore,ReplyCore))`, `server-watch:v3\0`+`J((sid,RequestCore,WatchCore))`, avec le même préfixe commun ci-dessus. request_id est frais par tentative, sans changer les identités métier restaurées. Watch commence à0 puis incrémente exactement ; overflow, trou, doublon, ancien epoch ou mauvaise corrélation ferment avant publication.

Seules opérations : `StatusStops`, `WatchStops`, `RestoreRequiredStop`, `QueryStop`, `DeliverDurableStop`, avec les wire tags snake_case existants. Les Restore/slots/codes EnforcerError conservent DTO, identité, validation, propriétaire et échéance V1. Résultats : StatusStops, StopReserved avec replayed, StopStatus, StopAccepted ou Error ; aucun Register/Prepare/claim/start/export/cleanup/retrait/finalisation. Abandonner le waiter ne détruit pas les preuves et tentatives R8 déjà retenues.

Status3 est fermé : `epoch,root_binding,granted_role=stops_only,known_stop_slots,max_slots,progress,transport_health=healthy|degraded,clock_healthy,admission_available=false,ledger_state,retirement_count`. max_slots égale le cap attendu, y compris avant publication Watch. Le seul ledger produit est `Unverified{reason:recovery_transport_only}` ; le nombre de retraits est `Unknown{reason:ledger_unverified}`, jamais0 inventé. known_stop_slots est la taille de la map STOP, pas un inventaire du ledger ni un compte de workers terminés. Aucune projection Complete/Partial productive n'est approuvée par ce transport ; leurs engagements futurs restent à confronter au vrai ledger.

Le serveur retient racine/endpoint/socket/lease et revalide avant préface, après N3, avant Grant, avant dispatch puis pendant Watch ; une substitution coupe le canal sans abandonner les STOP connus. Le client recoupe annonce/epoch/root après Grant avant publication du cache, sans écraser un epoch plus récent. Deadline3 monotone6s depuis l'entrée handshake ; chaque IO3 reste bornée à3s et au budget restant, sans reset. Le temps de la préface commune est inclus dès classement N3 ; les timings2 restent inchangés. Framing BE32,4096 octets handshake et512KiB total opération, enveloppe incluse ; aucun scan/fsync/join dans handshake ni promesse de préemption d'un syscall synchrone. Watch garde poll100ms, silence3s, stagnation750ms sur frames reçues et canal lecteur4 ; ces limites ne prouvent aucun terminal moteur.

Le client Stop3 a sa version fixée à la construction et partage seulement les algorithmes privés STOP autorisés. Aucun upgrade ou conversion Stop→Full. Les écarts initiaux d'ancres après accept, forme V1 embarquée, UUID de préface3 et cap Watch ont leurs quatre RED puis GREEN dans le candidat1. Les21 fonctions V3 incluent les vecteurs MAC littéraux et la fragmentation avec budget restant positif ; compatibilité2, R9 et reprises multi-Arm ont leurs canaris séparés. Ceci ne qualifie ni la matrice slowloris complète, ni toute la boucle monitor/reconnexion, ni un acteur ou effet moteur. Ledger/Closing/join/retraite/fencing et G09 restent ouverts.

### Deux preuves initiales fermées

`UnusedPrepareRevoked` exige le même acteur original vivant, jamais restauré ou ObserveOnly, Prepare jamais consommé
et aucune réservation d'émission commencée. Révoquer Prepare sous son mutex ; Prepare gagné, même ACK perdu, interdit ce motif.
Le writer vérifie l'absence de claim dans le head confirmé, pas seulement `prepared_once=false`.
Un latch propre au LocalSlot révoque aussi tous ses anciens handles/guards, sans invalider le CancelGeneration partagé du job.
La route productive qui rend ce slot abandonnable avant claim manque encore : `arm_and_claim` enchaîne register puis claim.
Un échec, un Drop ou un retour ObserveOnly ne crée pas cette route ni sa preuve.

`DockerNormalCompletion` exige un contexte frais privé, non Clone/non Serde, de PrepareBound : même Arm/claim/emitter,
génération/nonce, garde passée, child CLI exact créé puis attendu normalement, invocation/profil/binaire qualifiés.
Stop, drain, timeout, EOF anormal ou transport perdu interdisent ce témoin. Une première politique peut limiter la branche normale à 0,
mais sortie 0 seule, résultat public, détachement ou `ManagedPrearmedExecution` Ok ne suffisent jamais.
Le writer recoupe le terminal frais du même OwnedContainerSnapshot et la paire requête/réponse centrale ACKée exacte,
pour le worker ou le helper concerné. Terminal chargé ou quota déjà libre ne remplace pas cet échange.
Le runtime doit produire un témoin privé consommé une seule fois ; ce constructeur et la qualification Docker restent absents.
Podman et un programme fixture n'héritent pas de cette preuve Docker ; le fencing général des starts ambigus reste obligatoire.

### Identité, engagements et domaines cibles

Les nouveaux records seront fermés, versionnés et bornés, avec JSON canonique et UUID/hashes complets vérifiés.
L'identité lie scope/installation/job/intention, contrôle/origin/tenant/namespace, Arm/reçu et registration complète,
epoch original/génération, sujet/worker/attempt, CID, autorité, invocation/profil et empreinte de configuration.
Elle porte retirement_id et motif fermés ; le motif normal ajoute claim_operation_id/emitter_id exacts, sans secret/env/sortie brute.
Les noms suivants décrivent des **formats cibles**, pas des types ou des API disponibles :

| Record cible | Domaine distinct |
|---|---|
| Identité / engagement de preuve | `ai-guard:broker-retirement-identity:v1` / `ai-guard:broker-retirement-evidence:v1` |
| GenesisCore / sceau du rôle | `ai-guard:enforcer-retirement-genesis:v1` / `ai-guard:enforcer-role-seal:v2` |
| Floor / delta / entrée projetée | `ai-guard:enforcer-ledger-floor:v1` / `ai-guard:enforcer-retirement-delta:v1` / `ai-guard:enforcer-retirement-entry:v1` |
| ClosingAcceptance / tombstone | `ai-guard:enforcer-closing-acceptance:v1` / `ai-guard:enforcer-retired-tombstone:v1` |
| FinalizationCore / reçu | `ai-guard:enforcer-finalization-core:v1` / `ai-guard:enforcer-retired-receipt:v1` |
| Join réel / quiescence récupérée distincte | `ai-guard:enforcer-joined-quiescence:v1` / `ai-guard:enforcer-recovered-quiescence:v1` |
| Checkpoint V5 encapsulant V4 | `ai-guard:broker-checkpoint:v5` |

Graphe sans cycle : GenesisCore→genesis_hash/sceau ; ClosingAcceptance→tombstone→FinalizationCore→reçu→entrée/head.
ClosingAcceptance ne contient pas le hash de sa future tombstone ; core et reçu ne contiennent pas le hash de leur futur head.
Le head accompagne séparément la réponse authentifiée. Un digest n'authentifie pas un reçu : seul le client privé ayant vérifié
session/rôle, epoch et corrélation Arm/retirement_id, identité du ledger et engagements exacts peut produire son wrapper vérifié.
Ni JSON isolé ni Pending chargé ne reconstitue une permission vivante. Plans1–3, checkpoints V1–V4 et domaines historiques restent inchangés.

V5 encapsule V4 sans réencoder les anciennes révisions. Une chaîne ayant rencontré V5 ne peut plus produire de successeur V1–V4 ;
les anciens binaires refusent V5 sans interprétation permissive. Les entrées de retrait broker ont un ordre stable append-only :
une seule entrée par Arm, retirement_id unique dans le job, aucune disparition ni réattribution.
Pending→Final est monotone et conserve l'identité, la preuve engagée et le même Pending ; aucun passage direct à Final
ni Final d'un autre retirement_id n'est admissible. L'ACK d'audit final ajouté ultérieurement ne change pas le reçu gardien.

### Ordre vivant et capacité retenue

Notation : P = Pending avec intention audit-ACKée ; C = ClosingAccepted ; T = TombstoneCommitted ; Q = Finalized gardien ; F = Final V5.
Les états T/Q et leurs reçus ne sont pas des terminaux moteur ; un Pending sans ACK d'intention ne permet aucune acceptation gardien.

Le budget broker est distinct du budget ledger gardien : appliquer JournalLimitsV1 et les caps outbox/checkpoint existants à V5.
Le nombre de retraits par job reste borné par ses Arms ACKées et ses limites de bytes/entrées/révisions.
Avant de committer un nouveau Pending, réserver la place bornée de son Final et de l'événement final, en comptant la croissance réelle
et le pire cas des tailles fermées. Cumuler les réservations des retraits déjà promis ; un budget inconnu ou insuffisant refuse
ce nouveau Pending. Les Cancel conservent leur budget et leurs invariants existants.

1. Fermer les permissions locales du slot et vérifier/consommer la preuve initiale. Committer Pending V5 et l'intention d'audit
   dans l'outbox ; obtenir et persister le vrai ACK du même event_id/input_hash. Inconnu/absence d'ACK interdit Closing.
2. Le writer rend une AuthorizedRetirement privée single-use liée à ce Pending audité et à cette preuve consommée.
   Le gardien revalide rôle, epoch, registration, génération, motif et identité ; un vieux Pending sérialisé ne suffit pas.
3. Sous le même mutex que Prepare et toute réservation STOP mutative, Closing ne gagne que si aucune mutation STOP n'a **jamais été réservée**,
   sans effet mutatif en vol/inconnu et sans preuve dégradée. Un stop terminé n'efface pas sa réservation antérieure.
   Closing interdit irréversiblement Prepare, nouvel effet et reopen mutatif ; aucun IO, moteur, HTTP, fsync ou join sous ce mutex.
4. Hors acteur STOP, publier BeginClosing avec l'acceptation et le hash de tombstone attendu ; créer exclusivement les bytes engagés,
   synchroniser fichier/parents puis publier CommitTombstone. Résultat inconnu garde la même identité, la capacité et le fence.
5. Réveiller/sortir la boucle, attendre inspections/reopen possédés puis le **retour réel du JoinHandle retenu**.
   T/join bloqué n'autorise aucun reçu final ni capacité libre ; les autres STOP continuent. Annuler un futur n'est pas un join.
6. Après join, publier Finalize/Q durable et le même reçu rejouable ; seulement ensuite sortir l'acteur actif et décrémenter une fois.
   Q/ACK perdu se consulte/rejoue sans nouveau thread. Le statut Ready/actor_failed ne peut jamais effacer Closing.
7. Le broker commit Final V5 et l'événement final au head compatible courant, puis retire LocalSlot sous recovery_gate.
   Ses anciens handles restent révoqués ; audit final non ACKé reste visible et rejoue le même event_id, sans second événement logique.

Les event_id d'intention/final sont fixés dès Pending ; le payload final et son input_hash sont figés au commit Final avec le reçu réel.
Aucun reçu gardien ne libère un quota central, ne fabrique usage 0/terminal, ni ne remplace Cleanup et son autorisation/ACK distincts.
Closing gagné en mémoire puis FS inconnu ne revient pas Active et ne prouve pas une finalisation ; après perte de ce fait non engagé,
la reprise garde seulement STOP/fence, sans reconstruire le témoin initial depuis Pending. Cette convergence reste un écart ouvert.

### Lease, ledger engagé et indépendance des arrêts

La lease gardien spécialisée conserve **le même FD O_RDWR et la même description/flock/inode** jusqu'au dernier acteur/opération.
Pas de dup d'O_RDONLY pour écrire, de réouverture par chemin, replace/unlink du lock ou unlock/relock pendant append.
Owner/euid, mode 0600, nlink=1, ACL, parents et identité native restent vérifiés ; la factory neuve exige une preuve opaque de création exclusive.
Un répertoire apparemment vide ou `persistent_child(...,true)` sur existant ne produit pas cette preuve ; reprise ne régénère jamais auth/genesis.
La factory fixe role_id/init_id une fois, synchronise genesis/current puis sceau/Floor0 et parents avant toute Arm acceptée.
Initialisation interrompue : finir seulement la même init_id et les bytes exacts sous preuve retenue ; preuve perdue ⇒ diagnostic/migration,
jamais nouvelle initialisation automatique. Les anciens fichiers et l'autorité existante restent conservés.
La migration legacy exige inventaire quiescent, absence de starts/autorités inconnus et sauvegarde privée vérifiée, jamais un cutover implicite.

Le cache spécialisé partagé retiendra bytes bornés, metadata/digest, floors et état Parsed/Tainted ; PrivateFile général reste immuable.
Append sous mutex FS : revalider préfixe/longueur/identité, écrire à EOF attendu sans écraser, fsync, relire/adopter par ce même FD.
Write partiel/fsync inconnu ⇒ Tainted, suffixe/FD/flock conservés, Full/finalisation/append fermés ; aucun nettoyage vers un lock vide.
Les acteurs STOP ne prennent pas ce mutex. Listener/auth/transport restent disponibles sur leurs seules ancres structurelles vérifiées,
avant et pendant le scan isolé, même ledger/horloge malades ; horloge inconnue rend exigible le STOP original sans renouveler sa deadline.
Auth/racine/pair/socket/lease invérifiables restent de vrais blocages, sans nouvelle clé, takeover ni abandon déclaré sûr.
Full exige ledger Complete et une nouvelle connexion demandant explicitement Full ; aucune connexion StopsOnly ne gagne ces droits.
StopsOnly refuse Register frais/Prepare/claim/retrait initial/finalisation gardien/start/export/Cleanup ; le port Final V5 reste interne au writer.
Une erreur ledger ferme Full pour cet epoch ; aucune réouverture automatique après réparation, ni fallback V1 pour contourner ce refus.

Le sceau V2 non vide et Floor0/floors suivants seront cadrés BE32 dans ce même lock ; le legacy max_bytes=0 doit refuser ce rôle.
Genesis vide puis deltas contigus par Arm reconstruisent l'inventaire complet engagé, jamais le seul listage des tombstones présentes.
Chaque delta lie rôle/genesis/révision/prédécesseur/Arm/ancienne entrée attendue et une transition fermée BeginClosing, CommitTombstone ou Finalize.
Identité, preuve, ACK et retirement_id sont immuables ; trou, recul, ancien hash divergent ou transition arrière ferme Full.
Ordre durable : delta exclusif+sync fichier/parent → floor du delta+sync même lock → current-next/CAS current+sync parent → ACK.
Un floor en avance n'autorise que le CAS exact préparé ; suffixe complet exact se resynchronise, partiel/divergent se conserve et bloque.
Après T/Q, tombstone manquante/divergente est B1 : aucune reconstruction silencieuse ni nouvelle admission ; les STOP connus indépendants demeurent.
À C seulement, une tombstone encore absente peut être créée aux seuls bytes déjà engagés. Orphelin, floor déchiré et inventaire incomplet sont diagnostiqués.
Le rollback du seul ledger est détectable par le floor retenu ; le rollback cohérent de toute l'autorité exige un ancrage externe absent.

Caps cibles : 1024 Arms de retrait, 1 Gio logique cumulé, 4096 révisions, 8 Mio/record ; lock : 4096 floors, 2 Kio/frame, 8 Mio cumulés.
Compter les bytes réels de tous records, BE32, pointeurs, suffixes et lock ; ces maxima ne sont pas simultanément garantis.
Avant BeginClosing, réserver aussi les deux deltas restants, tombstone/floors/pointeurs de toutes les finalisations déjà promises.
Budget inconnu/saturé refuse le nouveau retrait avant Closing, sans éviction d'un retrait accepté ; ENOSPC/EIO ne devient pas un succès.
Pas de prune/compaction/suppression automatique. La borne cumulative ne qualifie ni 100 workers ni une durée d'exploitation ; rétention coordonnée reste obligatoire.

### Reprise metadata-only et quiescence entre epochs

La projection cible classe chaque Arm RequiredStop, RetirementPending ou RetirementFinal sous le reader/lease déjà retenus.
Si ledger Complete, interroger/vérifier Q puis réaliser **P/Q→F avant Restore de cette même Arm**, recharger son head/projection,
restaurer seulement les RequiredStop restants, puis consommer une seule factory Full/Stops. Les autres Arms/jobs/profils avancent indépendamment.
Le port crate-private ne peut finaliser que le même Pending déjà audit-ACKé, sans binding secret/blob fournisseur/AcceptedJob métier.
Il conserve Cancel et V4 courants, reconnaît le replay exact ; CAS conflict recharge metadata et fusionne seulement ce résultat, sans refaire un effet.
Il n'ajoute aucun Pending/claim/start/export/Cleanup et ne convertit pas Stops en Full. Le backend Stops **doit** garder le port tardif exact,
indépendant de `require_starts()` ; un ACK tardif ne doit pas créer l'impasse B2 ni une seconde factory/mutex sur la même lease.

Ledger Partial : Full/finalisations fermés, couverture/phase/erreur explicites, inconnus≠0 ; les RequiredStop prouvés restent restaurables.
Closing/Q vérifié de cette Arm interdit un nouvel acteur de contournement ; un défaut B ne retire pas le canal de A/C.
Si fermeture invérifiable et STOP restauré, l'ancien Q ne peut pas ensuite justifier l'oubli de ce nouvel effet potentiel.
**E1 Closing/Q → E2 Partial avec Restore ou STOP possible → E3 Complete : ancien Q/R+ interdit si l'intervention n'est pas exclue.**
RecoveredPriorEpoch est une preuve distincte de Joined, jamais un join historique inventé : même lease scellée, Closing positif et exclusion
de tout détenteur/effet mutatif orphelin ou intervalle intermédiaire requis. Flock libre, PID absent, epoch différent et annonces ne prouvent aucun drainage.
Sans cette preuve, `InterveningStopNotExcluded` reste une finalisation BLOCKED à résoudre ; ne pas ajouter de constructeur récupéré permissif.

### Acceptation du chemin complet

| Régression exigée, à réaliser avec vrais producteurs/owners et IPC | Résultat attendu |
|---|---|
| Sceau/floor déchiré, Journal/Cancel A prouvé | Diagnostic Partial/identité Unverified, Full/final refusés, STOP A conservé ; pas de type futur requis au premier RED. |
| Unused original, Prepare perdu/rejoué, acteur restauré, claim présent/inconnu | Seul le vrai original éligible peut fermer ; aucun témoin depuis statut/JSON/ACK perdu. Route productive avant claim nécessaire. |
| Courses Prepare↔Closing et réserve STOP↔Closing | Un seul gagnant, latch irréversible ; aucune IO sous mutex ni retrait après STOP potentiellement réservé. |
| T avec join/inspection/reopen bloqué, Q puis ACK perdu | Capacité retenue avant vrai join/Q ; autres STOP progressent ; même reçu et un seul décrément au replay. |
| B1 : Q réellement produit puis seule tombstone B retirée ; rollback seul ledger | Défaut détecté avant Full, floor respecté ; A/C lisibles restaurées, pas de genesis de remplacement ni omission silencieuse. |
| B2 : broker mort après Q avant F ; blob absent, Cancel concurrent, ACK tardif | Même F/reçu/event_id avant Restore de l'Arm ; Cancel/head préservés, port tardif sous Stops/fence, pas de second writer. |
| E1/E2/E3, autre inode/role/genesis ou détenteur encore vivant | Aucun R+ depuis flock/annonce ; effet intervenu non exclu reste BLOCKED, autres cibles continuent. |
| Même FD/cache, fautes append/fsync, suffixes exacts/divergents, budgets | Tainted conservé sans remplacement ; STOP indépendant ; reprise exacte seule, bytes/réservations réellement comptés. |
| Journal/Writer/outbox proches des caps, plusieurs Pending et Cancel concurrents | Finals/événements déjà réservés restent publiables ; nouveau Pending refusé avant acceptation si budget inconnu/insuffisant, budget Cancel préservé. |
| V5→V1–V4, entrée disparue/dupliquée/réattribuée, Final sans le même Pending | Rejets de chaîne ; Cancel concurrent conserve les entrées V5 et les bytes/domaines historiques ; ACK d'audit final ne change pas le reçu gardien. |
| 400 retraits/1200 deltas, compatibilité V1–V4/plans1–3, vrai CLI et ancien binaire | Format/budget et refus du rôle scellé vérifiés ; ni preuve 100 workers ni qualification Docker déduite du libtest. |

Tests confiés à Luna après gels et revue indépendante des frontières sûreté/audit/lancement ; programmes Rust fixtures nommés comme tels.
Le premier motif utilisable exige tous les maillons unused→Closing→join→Q→F et sa reprise, pas seulement un parseur ou un nouveau type.
DockerNormalCompletion qualifiée, fencing général, migration/restauration active, stockage/powerloss, rétention et matrice production restent obligatoires.
Ce contrat n'autorise ni installation OS/infrastructure ni arrêt d'anciennes ressources ; préserver leurs données, preuves, clés, gardiens et volumes.
