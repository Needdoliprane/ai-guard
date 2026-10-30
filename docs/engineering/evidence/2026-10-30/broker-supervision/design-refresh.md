# Actualisation du design de supervision durable

Date : 30 octobre 2026. Propriétaire : `/root/broker_supervision_design_refresh`.

## Livrable

Nouveau document seulement : `/Users/louis/Documents/ai-guard/docs/engineering/BROKER_SUPERVISION.md`, 101 lignes, environ 2 600 mots. SHA-256 : `afd625c2daaaf74f52f70ba29996d2f69b2064b0e0951cf2ff9d1095eb91e4cb`.

Statut explicite : DESIGN proposé, non approuvé, non implémenté. L'archive du 29 octobre, AGENTS.md, DECISIONS.md, CURRENT_STATE et tout code restent inchangés par cette tâche. Aucun service OS, test, runtime, compte distant ou secret réel consulté/modifié.

## Divergences corrigées et choix proposés

- L'ancien design voulait réutiliser « RunPlan v3 » pour la supervision. La v3 livrée désigne désormais les fournisseurs figés : proposition d'une enveloppe de supervision versionnée séparément, sans altérer sens, JSON/hashes, claims ni lecture des plans 1/2/3.
- L'ancien design relisait une source fournisseur à la reprise. Pour un plan3, reprise du binding privé existant et de son digest uniquement ; aucune relecture .env, rotation implicite, résolution Auto ou basculement abonnement/API.
- Les cinq décisions structurelles antérieures sont reprises dans une table D1–D5 avec recommandation : même binaire/service permanent ; arrêt déjà autorisé pendant panne d'audit ; échéances/latences mesurables ; compatibilité/migration explicite ; fencing des starts ambigus avant toute clôture.
- IPC sous UID propriétaire mais authentification runner/tenant distincte, profils/racines autorisés, socket/frames bornés ; aucune identité choisie par champ libre, aucun fallback direct/auto-daemon, aucune clé dans jobs publics/outbox/logs.
- Scheduler d'arrêt indépendant des HTTP/audit/captures/modèles ; ciblage strict CID/autorité/config ; journal/outbox bornés, durabilité préalable au claim et panne locale traitée sans inventer d'ACK, usage ou quota.
- Les limites TTY et la perte du client sont incluses, sans prétendre livrer le réattachement ni remplacer le TUI par du batch.
- Installation OS explicitement séparée de l'implémentation ; aucun sudo, linger, chargement launchd ou unité existante remplacée.

## Points à trancher/qualifier explicitement

1. Fencing D5 : aucun `rm` réussi ou `NotFound` n'est traité comme preuve d'annulation d'un ancien start en vol. L'adaptateur doit produire une vraie preuve avant clôture ; sans cela, cas ouvert, enforcement actif et quota retenu.
2. macOS : KeepAlive ne prouve pas la récupération d'un broker vivant figé (SIGSTOP/blocage). Le mécanisme effectif de détection/récupération doit être conçu/qualifié ; cette proposition n'invente pas un watchdog déjà disponible.
3. Budgets initiaux proposés, non mesurés : tick250ms, déclenchement stop≤1s si service/moteur disponibles, reprise crash≤10s sous gestionnaire, observation nominale≤15s après stop. Pas de succès par percentile ; sommeil, moteur indisponible et gestionnaire désactivé restent des pannes visibles, pas une promesse de temps réel.
4. Schéma/protocole exacts d'armement et négociation de capacité à détailler avant code dans le plan racine. Ne pas étendre silencieusement les types stricts actuels ni réinterpréter v3.
5. La clôture « arrêté, mesure indisponible » avec quota libéré est une décision distincte ; ce design ne l'ajoute pas implicitement. Bytes/inodes et réserve de preuves restent à qualifier.

Ces points ne réduisent pas le périmètre produit et restent des conditions de qualification, même si la structure est acceptée.

## Vérification de cette tâche

- PASS documentaire : lecture des autorités AGENTS/RTK/architecture/règles/état/tenancy/usage/plan/stratégie/gates, archive historique et code directement concerné (`runner.rs`, `runner/managed.rs`, `control.rs`, documentation fournisseur).
- PASS liens locaux : `/tmp/ai-guard-engine-docs-scan/links docs/engineering/BROKER_SUPERVISION.md` retourne `relative_links_checked=10 missing_targets=0 markdown_files=1`. Outil préexistant, aucun build ; contrôle d'existence seulement, pas des fragments/URLs externes.
- Sources primaires publiques recoupées : Tokio `UnixStream::peer_cred`, systemd.service, guide Apple launchd, noyau Linux timekeeping et boot_id, headers/source XNU pour `mach_continuous_time`/timebase et `bootsessionuuid`. La page Apple API impose JavaScript et son lien Markdown retournait404 ; le header officiel a été lu à la place. Aucune citation de source secondaire nécessaire.
- NOT_RUN : tests Rust/front/navigateur, unités OS, moteurs réels, API fournisseur, qualification et charge. La tâche est exclusivement documentaire.

## Formulation de soumission proposée au root

> AGENTS.md demande de « soumettre un changement structurel motivé avant de le réaliser ». Autorises-tu l'implémentation du broker Rust permanent décrit dans `docs/engineering/BROKER_SUPERVISION.md` (même binaire, IPC privé, échéances durables), sans installer de service OS ni modifier une infrastructure à ce stade ?

Faire relire la constructibilité de ce document avant de soumettre cette question. Il n'a pas encore reçu de revue indépendante ; ce rapport n'est pas une approbation de sa faisabilité complète. Une réponse positive ne qualifie aucun résultat de test ni n'autorise une destruction/restauration de données existantes.

## Addendum — corrections après première revue de constructibilité

Le rapport indépendant `/tmp/ai-guard-broker-supervision-buildability-review.md` a relevé BSR-01/02/03. Le nouveau gel documentaire est `9cc0ef1ef72f8197f3ebb0afad3f5924afa89b51b3274a24a2215178d4c5f9ce`, 123 lignes, environ 3 900 mots. Le hash et les tailles de la section initiale restent ceux du premier gel historique.

- BSR-01 : le `RunPlan` courant est identifié comme checkpoint mutable, sans prétendre au hash permanent de ses octets. `ExecutionIntentV1` projette explicitement les seuls champs initiaux stables, exclut `managed`/FD et utilise un domaine/canon JSON versionnés distincts. `ExecutionCheckpointV1` porte révision et référence d'intention ; publication de révision/pointeur confirmée après fsync. Rejeux recherchés avant résolution ; ACK après binding/intention/checkpoint0/marqueur durable. Préparation partielle sans intention complète : pas de nouvelle résolution sous le même ID ni de faux ACK accepté. Hashes d'autorisation existants préservés.
- BSR-02 : le CLI n'est plus le seul responsable de restauration. Gardien Rust local lancé par le CLI depuis le même binaire, hors groupe de vie du client et hors cgroup/job du broker ; FD physique et attributs initiaux retenus avant raw, copie de secours aux deux survivants. Grant limité/UID+runner, preuve native du pair/processus/terminal, refus si non disponible, environnement/FDs fermés sans secrets. Le gardien ne lit aucune frappe et est seul émetteur des sorties filtrées. Mort/EOF/lease : fermer génération, arrêter writes/backlog, restaurer puis relire termios avant ACK/restauration coopérative. Un foreground inconnu n'est pas forcé ; aucune promesse si tous les détenteurs du FD sont tués ou le terminal disparaît. Recette explicite SIGKILL du PID client pendant raw et témoin d'un autre terminal.
- BSR-03 : aucune attente du fsync local ou du mutex mutable sur le chemin de stop. Enqueue borné non bloquant, writer FS isolé, snapshot de stop immuable et capacité runtime réservée. Writer bloqué/queue pleine empêche de nouveaux starts, mais l'arrêt déjà armé est tenté sans nouvel ACK de journal. Une observation perdue en mémoire n'est pas déclarée durable ; pas de cleanup/quota fictif. Recette fsync suspendu/checkpoint mutex occupé ajoutée.
- Le texte « ces choix autorisent une implémentation » est maintenant conditionné à l'approbation explicite. D1 inclut le helper gardien dans le périmètre structurel ; l'accord code ne vaut pas installation OS. D5 et watchdog restent ouverts comme précédemment.

Contrôle statique après corrections : même commande de liens, 10 cibles locales présentes, zéro absente. Aucun code, service, test, fichier de secret ou infrastructure modifié/consulté. Sources primaires complémentaires lues : manuel Unix Linux pour passage FD, header `bsd/sys/un.h` Apple pour identité native du pair, référence POSIX `tcsetattr` pour restauration/readback. Ces références ne qualifient pas l'implémentation proposée sur les plateformes supportées.

### Question de soumission actualisée (remplace la formulation initiale)

> AGENTS.md demande de « soumettre un changement structurel motivé avant de le réaliser ». Autorises-tu l'implémentation du broker Rust permanent **et de son gardien TTY local du même binaire**, décrits dans `docs/engineering/BROKER_SUPERVISION.md`, avec IPC privé et échéances durables ? Cette approbation ne chargera aucun service OS et n'autorisera aucune opération d'infrastructure ou destruction de données ; leur installation restera une étape opérateur distincte.

Relecture indépendante du delta demandée au root/reviewer ; les points BSR ne sont pas déclarés fermés par leur auteur avant cette relecture. Le statut demeure DESIGN non approuvé/non implémenté et tous les tests restent NOT_RUN.
