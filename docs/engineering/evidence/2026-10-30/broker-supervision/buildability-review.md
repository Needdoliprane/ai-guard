# Supervision broker — revue indépendante de constructibilité

30 octobre 2026. Document examiné : `docs/engineering/BROKER_SUPERVISION.md`, SHA-256 `afd625c2daaaf74f52f70ba29996d2f69b2064b0e0951cf2ff9d1095eb91e4cb`, avec `/tmp/ai-guard-broker-design-refresh.md`, AGENTS, architecture/règles et code directement concerné. L'archive du 29 octobre reste un design non livré ; elle n'est pas une preuve d'exécution.

**Avis : orientation compatible avec l'architecture et constructible sous conditions, mais contrat pas encore assez précis pour autoriser aveuglément l'ensemble de l'implémentation.** Trois points de conception ci-dessous doivent être arrêtés dans le document/contrat avant le code correspondant. Le fencing du start ambigu et la reprise d'un broker vivant figé restent, à juste titre, des dépendances de faisabilité non résolues. Une approbation de direction ne doit pas les présenter comme déjà réalisables ou qualifiées.

Revue exclusivement en lecture. Aucun test, service, installation OS, runtime, API ou modèle lancé ; aucun credential/source `.env` ouvert ; aucun fichier produit édité. Les sources externes citées par le concepteur ne sont pas requalifiées indépendamment ici. Le contrôle de liens existant a trouvé 10 cibles locales présentes, sans vérifier fragments/URLs externes.

## BSR-01 — P2 conception : définir la partie immuable du plan avant de lui lier l'enveloppe

Références : proposition lignes 20, 37 et 69 ; `src/runner.rs:308–328`, `src/runner/managed.rs:407–439`, `:562–588`.

L'enveloppe est dite séparée et liée à « l'identité/hash exacts du plan existant ». Or `RunPlan` contient `managed`, un checkpoint **mutable** : chaque transition ajoute `pending`, puis remplace le worker/révision et retire `pending`, en réécrivant le plan entier via `PlanState::update`. L'approbation change aussi la phase de demande. Le hash de la sérialisation complète d'un fichier de session n'est donc pas une identité immuable de job.

Séquence démontrable sans test : créer l'enveloppe avec hash du plan initial → `mutate` écrit `managed.pending` avant RPC → le fichier JSON ne correspond déjà plus au hash → une reprise qui vérifie ce hash rejette le job légitime. Réécrire automatiquement le hash de l'enveloppe à chaque modification supprimerait au contraire son sens d'engagement immuable et introduirait une nouvelle transaction/crash window.

À préciser : projection canonique versionnée et domaine de hash des champs immuables (identité de soumission, invocation/options autorisées, binding fournisseur opaque et digest privé, contexte tenant/origin/autorité), ou copie immuable du plan initial distincte du checkpoint courant. Définir séparément les révisions/liaisons du checkpoint, de l'approbation et de l'armement. Ne jamais conserver aveuglément un hash d'un fichier destiné à évoluer.

L'ACK durable doit aussi avoir un point précis : pour une soumission plan3, résolution et binding doivent être figés une seule fois avant l'ACK auquel la promesse « aucune relecture de source après ACK » s'applique. Après perte de réponse, lookup de l'identité durable avant toute nouvelle génération de binding/résolution ; crash entre persistance du binding et persistance du job à traiter sans accepter un autre contenu pour la même identité.

Régression requise : submit → perte ACK → reprise → approbation éventuelle → mutations/checkpoint → crash/reprise, identité immuable stable ; changement d'argv/binding/tenant rejeté, transition légitime acceptée, zéro nouvelle résolution Auto.

## BSR-02 — P2 conception : restauration du terminal physique après SIGKILL du client non définie

Références : proposition lignes 75, 89–90 et 96 ; `src/runtime/terminal.rs:535–605`.

Le design attribue restauration termios et clavier au CLI, runtime/PTY au broker. Le code actuel restaure le terminal dans `TerminalGuard::restore/Drop`, avec les attributs sauvegardés et un FD physique retenu. Si le CLI passe son terminal en raw puis reçoit SIGKILL, son destructeur ne s'exécute pas. Posséder le PTY du transport moteur dans le broker ne fournit ni le FD du terminal physique ni ses attributs antérieurs. L'arrêt autonome du worker n'implique donc pas, à lui seul, « terminal rendu propre ».

À décider avant l'intégration TTY : propriétaire survivant de la restauration (FD physique retenu et état sauvegardé transmis sous contrôle, ou gardien local indépendant du CLI, ou autre mécanisme justifié). Définir une lease d'attach, le handshake avant passage raw, l'ordre arrêt des écritures → restauration → restitution, et le comportement si le terminal disparaît ou change de foreground/propriétaire. Ne pas restaurer tardivement un terminal déjà repris par un autre utilisateur/processus. Le cas SIGKILL doit être relié explicitement au critère de restauration ; une simple RAII dans le client ne suffit pas.

Régression requise : vrai PTY physique de test, client raw tué brutalement avant/après ACK d'attach, daemon vivant ; état termios/restauration contrôlé avant réutilisation du terminal, aucune écriture après restitution, worker sous échéance indépendante et aucun nouveau start. Aucun modèle nécessaire. Cela ne demande pas un réattachement déjà qualifié.

## BSR-03 — P2 conception : l'intention fsync préalable ne doit pas bloquer l'enforcement

Références : proposition lignes 46, 59 et 63–65 ; boucle actuelle `src/runtime.rs:1069–1072` pour le défaut d'attente déjà connu.

La proposition promet un ordonnanceur indépendant de l'HTTP/audit, puis prévoit « avant enforcement, écrire si possible une intention fsync ». Le cas disque plein/readonly est traité, mais une écriture/fsync qui **ne rend pas la main** ne produit pas d'erreur permettant ce repli. Si l'ordonnanceur ou un verrou qu'il attend porte cette écriture, l'arrêt peut ne jamais être demandé même avec moteur disponible. Le même problème vaut pour inspection préalable bloquée ou file de sous-processus saturée.

À préciser : armement durable préalable = autorité suffisante d'arrêt ; l'envoi stop après échéance ne doit attendre ni nouveau fsync, ni RPC audit, ni mutex détenu par ces opérations. Définir la capacité réservée et les files/locks entre ordonnanceur, journal, inspection et moteur ; délais/bornes d'IO et politique quand une tâche bloquante n'est pas annulable. Une simple `timeout` autour d'une future ne prouve pas la libération d'un verrou ou d'un thread effectuant le syscall. Les observations et la preuve d'intention peuvent rester en attente selon le contrat déjà annoncé, sans inventer leur durabilité.

Régression requise : journal/ACK bloqué (pas seulement retour ENOSPC), captures/export sous pression et inspection lente, puis échéance d'un témoin ; arrêt ciblé soumis dans le budget qualifié ou état de défaillance effectivement détecté. Ne pas définir « service disponible » par le seul fait que l'arrêt a réussi, ce qui rendrait le budget invérifiable.

## Dépendances explicitement ouvertes à conserver dans la soumission

1. **D5 : fencing moteur du start ambigu.** Le document a raison de ne pas assimiler `NotFound`, `Created` ou succès de `rm` à une preuve que l'ancien RPC ne peut plus démarrer. C'est un verrou de qualification de ce cas, pas un détail à déclarer réglé après ajout du daemon. Préciser la capacité attendue par adaptateur, la procédure de preuve et un point d'arrêt de l'intégration si aucune capacité ne peut être démontrée. Une phase de définition/expérimentation ciblée peut être autorisée ; la réussite n'est pas présumée.
2. **Broker vivant figé et présence après logout.** D1 apporte l'indépendance vis-à-vis du CLI, pas la récupération automatique de toutes les pannes du service. Le document reconnaît correctement le manque de mécanisme prouvé sous macOS. Avant de promettre la recette SIGSTOP/crashloop/logout, choisir le mécanisme concret, son propriétaire, son accès et son interaction avec le verrou/fencing. Si un composant indépendant ou des privilèges d'installation deviennent nécessaires, les présenter comme tels dans la décision structurelle, sans installation implicite.
3. **Contrat central d'armement.** Nouveaux DTO/migration/négociation et refus côté serveur du claim sans armement doivent précéder l'activation du service ; pas de simple booléen privé. Définir les crash windows entre armement privé, reçu central et claim. Les helpers import/scan/export ont aussi besoin d'une identité et d'une durée propres, sans utiliser leur armement comme permission de commande arbitraire. La proposition les couvre comme exigence, pas encore comme schéma livré.
4. **Compte de contrôle de confiance.** L'authentification runner/tenant sur IPC est nécessaire et distincte du pair Unix, mais ne transforme pas le même UID propriétaire de tous les credentials en adversaire isolé par tenant. Le test « viewer sous bon UID » doit vérifier l'absence d'élévation par l'API ; il ne peut prouver qu'un processus malveillant disposant de tous les fichiers du compte ne lise jamais un token runner. Cette limite doit rester explicite, sans ajouter de compte/ACL/secret par magie.

## Éléments cohérents à conserver

- Le même binaire Rust dans le périmètre de contrôle, le gateway comme autorité durable par tenant, aucun socket/credential supplémentaire au worker/dashboard, aucune nouvelle base souveraine : conforme aux frontières validées sous réserve de décision structurelle.
- Numéro plan3 préservé ; réouverture du binding privé figé par digest, sans recharger `.env`, choisir Auto à nouveau ni basculer abonnement/API. Arrêt sans credential fournisseur. Les types stricts et historiques ne sont pas étendus silencieusement.
- Claim consommé jamais réarmé ; identité de job/tenant/origin/autorité/CID liée ; aucun kill par nom ou adoption de ressource divergente. Panne d'audit ne permet pas un nouveau start ; arrêt préautorisé distinct d'un nouvel allow.
- Horloge continue/boot séparée de l'UTC et de l'usage ; pas de nouvelle fenêtre lors de la reprise ; compteurs inconnus non inventés et quota retenu sans preuve terminale suffisante.
- Réserves disque/inodes, conservation des preuves et quota/usage explicitement séparés ; pas de suppression pour obtenir un test vert.
- Installation systemd/launchd indépendante de l'implémentation, unités existantes préservées, aucune action OS autorisée par le seul document. Matrices Docker/Podman et macOS/Linux distinctes, recette A1/A2/B1 utile et charge100 séparée.

## Avis pour la décision

Ne pas soumettre comme une « solution entièrement constructible et prête à coder » sans les choix BSR-01/02/03. Soumettre une direction structurelle concrète après leur clarification, en indiquant que la preuve D5 et la récupération d'un broker figé sont des conditions non satisfaites de la livraison complète. La décision du mainteneur ne remplace ni ces preuves ni la revue du contrat détaillé.

Statuts : revue documentaire **effectuée** ; conditions de conception **ouvertes** ; implémentation/services/tests/qualification **NOT_RUN**. Les trois points numérotés sont des défauts ou ambiguïtés de proposition, pas des vulnérabilités d'un daemon déjà livré.

## Addendum — relecture du design corrigé, 30 octobre 2026

Premier gel ci-dessus conservé : `afd625c2daaaf74f52f70ba29996d2f69b2064b0e0951cf2ff9d1095eb91e4cb`. Nouveau gel de `docs/engineering/BROKER_SUPERVISION.md`, hash vérifié : `9cc0ef1ef72f8197f3ebb0afad3f5924afa89b51b3274a24a2215178d4c5f9ce`. Addendum du concepteur `/tmp/ai-guard-broker-design-refresh.md` recoupé. Cette relecture ne modifie aucun fichier du dépôt et n'exécute aucun test/service/runtime.

**BSR-01 fermé au niveau de la conception** — lignes 20, 37 et 73–83. L'identité permanente porte désormais une intention typée et canonique, à domaine versionné, distincte du checkpoint révisionné et des hashes d'autorisation existants. Le texte exclut explicitement le JSON mutable `RunPlan.managed`. La réservation d'opération précède la résolution ; le rejeu consulte cette réservation avant source/Auto ; ACK après binding, intention, checkpoint initial et marqueur durables. Une préparation interrompue ne permet pas de résoudre à nouveau sous le même identifiant. Approvals, transitions et migrations ont leurs révisions sans réarmer le claim. Le scénario initial de hash périmé n'est plus le contrat proposé.

**BSR-02 fermé au niveau de la conception** — lignes 17 et 87–97. Le helper gardien fait maintenant partie du changement structurel soumis : lancé par le CLI, indépendant de sa durée de vie et du job broker, avec FD/attributs conservés avant raw et secours chez les survivants. Le gardien n'a pas un rôle runner général, ne lit pas le clavier et constitue l'unique écrivain physique des sorties filtrées. Grant borné, génération, preuve native de pair/naissance, validation terminal et prepared/active précèdent raw/claim. La restitution révoque les frames et le backlog avant reset/restauration/readback ; elle n'attend ni HTTP ni fsync. Le texte refuse une restauration forcée sur nouveau foreground et ne promet pas de survivre à la mort de tous les détenteurs. La solution explicite reste à implémenter et tester ; elle ne repose plus sur une hypothèse de RAII survivant au SIGKILL.

**BSR-03 fermé au niveau de la conception** — lignes 18, 44–47 et 63–69. L'armement durable antérieur suffit à autoriser l'arrêt ciblé ; sa production reste obligatoire avant claim. Le nouveau journal d'application utilise un enqueue non bloquant et un writer isolé, sans verrou partagé sur le snapshot de stop ni attente de fsync. File pleine/syscall bloqué interdisent de nouvelles admissions, sans retirer l'autorité d'arrêt déjà acquise. Les observations perdues ne sont jamais déclarées durables ; quota/cleanup restent subordonnés au contrat central. Pas de contradiction nouvelle entre audit obligatoire avant lancement et arrêt préautorisé pendant panne.

La recette lignes 113, 117–118 couvre désormais les trois défauts : fsync réellement bloqué, évolution des checkpoints/perte ACK et SIGKILL/foreground/absence d'écriture après restitution. Les détails de contrat doivent encore préciser les schémas canoniques, les canaux/FD et l'exclusion des secours concurrents, ainsi que la stratégie d'IO et les budgets mesurés. Ce sont des exigences d'implémentation de la solution choisie, pas des preuves déjà obtenues. Le même UID de contrôle reste une frontière de confiance ; les rôles API ne constituent pas une isolation hostile entre processus disposant des fichiers de ce compte.

**Avis actualisé : proposition suffisamment concrète pour être soumise au mainteneur comme direction structurelle conditionnelle.** Aucun nouveau blocage documentaire établi dans ce delta. La question doit inclure le broker permanent **et le gardien TTY**, sans implication d'installation OS. D5/fencing et récupération d'un broker vivant figé sous macOS restent ouverts : leur faisabilité et leurs preuves sont nécessaires à la livraison, même après une approbation. Cette revue ne vaut ni acceptation d'un risque critique ni déclaration de code qualifié.

Contrôle existant des liens relancé sans build : 10 cibles locales présentes, zéro absente ; ancres et URLs externes non vérifiées. Statuts actuels : BSR-01/02/03 **FERMÉS documentairement** ; approbation du mainteneur **EN ATTENTE** ; implémentation, installation, tests et qualification **NOT_RUN**. Le verdict du premier gel reste historique.
