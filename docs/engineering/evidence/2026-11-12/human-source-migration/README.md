# Migration des sources humaines — preuve logicielle du 12 novembre 2026

**PASS borné : 17 parents Rust exacts, build lib et check toutes cibles.** Ce lot livre des ports internes de maintenance et de démarrage ainsi qu’un coordinateur A+B+Auth ; il n’active ni commande utilisateur de migration, ni Ready, ni approbation ou lancement humain. Aucun gate global ni fingerprint d’audit historique n’est fermé. [Sélecteurs, RC et empreintes](TESTS.json).

## Comportement vérifié

Les fixtures utilisent de vrais fichiers SQLite privés. Les états tenant 14 et Auth 5 positifs viennent des vraies sauvegardes, conversions et commits ; ils ne sont pas fabriqués par une DDL 14 manuelle. Le coordinateur acquiert les writers Auth puis tenants triés avant ses premiers effets, conserve un journal de progression et ne rend son bundle interne qu’après vérification de la fermeture et des incarnations.

Les sept parents openers/Auth vérifient la maintenance/reprise d’anchor, les tokens étrangers et la contention, l’incarnation courante avant rétention, le refus d’un tenant/source étranger sans empoisonner le bon owner, le floor d’un commit connu après expiration réelle puis reprise avec epoch neuf, la rétention du head observé par les deux getters et le CAS d’incarnation Auth après redémarrage.

Les dix parents du coordinateur vérifient la vraie migration A+B+Auth avec mutation et lecture sous epochs, le dernier writer occupé sans effet, la reprise après un premier commit réel, le refus de fermeture partielle, les clés sources manquantes ou supplémentaires, le périmètre du budget Auth et une comparaison unitaire de timestamps à la nanoseconde, le journal divergent, un backup main préexistant et les trois sidecars préexistants. Les backups et sources sont conservés lors des refus. Les scénarios ne qualifient pas une connexion OIDC réelle ni une approbation humaine.

Le parent de clés sources ne remplace pas un essai de substitution physique d’une base ou d’un fichier de configuration. Le parent de maximum teste le comparateur temporel et un maximum de valeurs DateTime ; il ne construit pas trois floors durables distincts A/B/Auth. Ces deux validations restent à ajouter avant activation et ne sont pas déclarées exécutées par les 17 PASS.

## Source et environnement

Base publique e507619190fc6c12ddd4a5f1b49b5c4a2152ca61 plus onze postimages applicatifs (six nouveaux chemins), sans les 134 postimages Full3 privés. Les 2 578 autres blobs et modes publics sont identiques. Gel exécuté : 9e7d14f25ea80634132d24d89f080ac1d92465814de0360b2bacd337fca96b0e, 30 044 160 octets, 2 589 fichiers réguliers 0644. Les documents de suivi ajoutés à la publication ne sont pas présentés comme des entrées de ce gel.

QA Linux ARM64 épinglée, Rust 1.99.1, UID 1000, réseau absent, racine et deux binds en lecture seule, capabilities supprimées, NNP/seccomp 2, quatre CPU, RAM 9 Gio, swap 0, PID 128 et scratch 4 Gio. Cargo reste frozen/offline, jobs 1 ; les 17 parents compilés sont invoqués exactement, chacun borné à 30 s, avec arrêt au premier défaut. Le check toutes cibles n’est exécuté qu’après leurs 17 PASS. Aucun fournisseur IA, identité distante ou moteur worker n’a été appelé. Cette QA dans Docker n’est pas une qualification de l’adaptateur Docker.

Root a vérifié 363 pièces runtime, 2 615 pièces statiques et les 2 589 contenus/modes postrun. Export vivant fab83a6a75a81600d5b90e97db98a88019f93a717fdd0f72f818569f74f745d0, 30 054 400 octets, sans dépassement de 32 Mio. Fermeture du CID exact RC 0, absence finale confirmée, aucun OOM ; pic de 6 339 534 848 octets sur 9 663 676 416. ExitCode137 est celui du holder après stop borné, et ne remplace pas les RC0 des tests/check. Reçu indépendant Root 23361cd9c4626014be019fccfc95c93cf683d9019225c219dce123000d376941 ; les originaux restent privés et ne sont pas copiés dans cette preuve publique.

## Reproductions et corrections, sans effacer les échecs

- F1 : le getter de révision renvoyait la tête 15 mais conservait le témoin 14. Ce RED réel concernait la branche révision atteinte ; la branche floor mural n’avait pas été exécutée sur le baseline. Le correctif retient le head du snapshot vérifié après commit connu, avant les rechecks fallibles. Le parent final termine les deux branches. Le premier essai F1 avait échoué en compilation sur une visibilité privée : ce n’était pas un RED du comportement.
- U1 : une première tentative adoptait un backup exact préexistant et écrivait une intention. Le RED à la première assertion journal vide a précédé la correction. Le préflight couvre maintenant main, -wal, -shm et -journal avant intent sous tous les writers ; une première tentative conserve la création exclusive. Une reprise avec intent durable garde les vérifications du même plan et des mêmes ancres. Les trois sidecars ont un GREEN final ; aucun RED sidecar antérieur n’est prétendu.
- Le premier GREEN7 a passé quatre parents puis a échoué sur un oracle incorrect : begin_close ferme les admissions, sans annuler les epochs déjà acceptés. Seul le test a été corrigé vers une expiration réelle après commit ; la règle de drainage est conservée. Les parents restants/check de cet essai étaient NOT_RUN.
- La première composition17 a passé sept parents puis a échoué sur une intention Login générique, correctement refusée par le budget Auth 4/5. Seule la fixture a été corrigée : elle conserve ce refus sans effet, puis appelle le vrai port Initial owned, avec principal OIDC et fingerprint synthétiques explicitement identifiés. Receipt=head, records+1 et fingerprint sont relus sous WRITE et READ ; aucune session admise n’est inventée. Les neuf parents restants/check de cet essai étaient NOT_RUN.

Les revues source indépendantes F1, U1r2 et des deux corrections d’oracle sont favorables dans leur périmètre ; elles ne remplaçaient pas l’exécution. Les rapports et postimages antérieurs sont conservés, y compris le REQUEST_CHANGES U1r1 sur le namespace incomplet.

## Portée encore ouverte

Le parcours humain complet demande encore le raccord des mutateurs Store/OIDC/RAM/login/refresh/logout/Drop/setup aux vrais owners, sources et epochs, le runner enregistré et ses réserves, l’approbation nominative, l’admission broker, puis les API/CLI/UI. Aucun nouvel endpoint ou flag utilisateur n’est annoncé par ce lot. Les guides utilisateur ne changent pas de contrat.

Restent NOT_RUN pour cette preuve : canari legacy concurrent spécifiquement bloqué (non réessayé), ancien binaire réel, macOS natif, OIDC/fournisseurs/CLI réels, workers Docker/Podman/Kubernetes, restauration multi-processus, charge de 100 workers, Clippy strict et suite complète du dépôt. Les 134 postimages runtime privés ne sont pas qualifiés par ces 17 PASS. La preuve ne ferme ni l’approbation humaine absente des audits, ni la médiation egress, ni les autres obligations de production.
