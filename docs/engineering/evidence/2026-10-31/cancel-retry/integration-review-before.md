# Revue indépendante — intégration Cancel exact / livraison STOP

31 octobre 2026. Lecture seule, worktree `broker-service`, delta contre `44e5ec3`. Périmètre gelé : client enforcer, BatchExecutor, server/jobs et nouveaux tests Cancel natifs/handler/checkpoint. Aucun Cargo, service, moteur, fournisseur ou cloud exécuté par le reviewer ; aucune source modifiée. La projection metadata a été relue séparément ; sa clarification EXACTJOB est conservée.

**Gel initial : un défaut P1 confirmé par chemin de source, accepté par l’orchestrateur. Nouveau gel : correctif relu, défaut de sélection de chaîne fermé au niveau source ; preuve Luna encore attendue dans cette revue.** Aucun autre P0/P1 confirmé. Aucune approbation automatique. Les compilations/tests Luna, y compris leurs échecs de harnais, restent des preuves distinctes.

## P1 — intention Cancel non retenue sur le dernier client failed d’une chaîne de récupération

Source initiale : `src/broker/enforcer/client.rs:374` vérifie et marque uniquement `self.0.slots`, puis appelle `self.active()` à `:392`. Or `register` (`:204`) retient les nouvelles cibles dans le client actif ; `recover_stops` ne recopie vers une génération suivante que les cibles présentes dans le client qui vient d’échouer (`:972`).

Séquence reproductible avec les coutures privées du test natif, sans moteur :

1. Établir une chaîne client racine C0 → remplacement C1 après une première récupération.
2. Retenir une nouvelle cible T dans C1 uniquement, avec son digest et le même Arc CancelGeneration que le job.
3. Marquer C1 failed et capturer son inventaire de récupération ; aucun C2 n’est encore publié.
4. Appeler `C0.cancel_retained(T, generation)` pour la commande durable exacte.
5. Le lookup de C0 ne trouve pas T. `active()` rencontre C1.failed et rend Unhealthy avant tout accès à T. Les marqueurs `retained_cancel` et `cancel_durable` de l’Arc T capturé restent false.

Conséquence : l’intention durable d’arrêt anticipé n’est pas conservée dans l’inventaire de reprise existant. `restore_slot` peut suivre sa branche ordinaire (`:1053`), restaurer ObserveOnly sans livrer Cancel ; pour un acteur NotReady, cette branche peut échouer avant Cancel et empêcher la publication du remplacement. Les reprises via C0 échouent encore avant de marquer T. L’échéance d’origine reste armée ; cela ne prouve donc pas une perte de toutes les échéances, mais viole la livraison/reprise du Cancel durable exact.

Correction minimale attendue : parcourir la chaîne bornée accessible en marquant chaque Arc existant correspondant avant de refuser sur son dernier client failed ; conserver les contrôles digest et identité d’Arc génération ; ne créer aucune cible dans un client après son fuse. La régression doit démontrer C0→C1, cible absente de C0, C1 failed, marqueur conservé sur l’Arc capturé, aucune insertion inconnue ni seed de génération. Le cas initial « seul client failed » ne couvrait pas cette chaîne.

**Après correction, gel client `83571728…39834`.** `active_for_retained_cancel` parcourt au plus 128 clients. Sur chacun, il inspecte uniquement les slots déjà présents, vérifie digest et Arc de génération, puis marque l’Arc avant de suivre replacement ou de refuser le dernier failed. Aucun insert ni seed n’est présent dans ce parcours. La création éventuelle reste dans `retain_registration`, sur le client actif, avec ses contrôles de fuse/capacité existants. Le test natif ajouté construit C0→C1 avec une cible présente seulement dans C1 failed, vérifie les marqueurs/stop sur cet Arc et les tailles inchangées des deux caches pour une cible inconnue. Le scénario de source initial ne peut plus atteindre son retour Unhealthy sans marquer T. Ce test vérifie la conservation du marqueur ; il ne prouve pas à lui seul une récupération complète de processus ou un arrêt runtime.

## Autres contrôles retracés sans défaut bloquant relevé

- `server.rs:923` distingue cache durable true et nouveau Cancel. Le replay confirmé n’appelle ni begin_cancel, ni append, ni le gate d’exécution ; il redemande la projection de la commande exacte. Un nouveau Cancel invalide sa génération avant identity_gate/status, puis persiste la commande avant livraison.
- Le cache de `jobs.rs:137` reste borné, refuse le pending concurrent et préserve Unknown après Io ; ce booléen représente la durabilité de la demande, pas une livraison réussie. Un append Io laissé pending rend Busy jusqu’à confirmation par status/recovery vérifié ; il ne produit ni nouvelle invalidation au retry ni faux ACK. Cette limite n’est pas annoncée comme une réparation automatique de tout résultat inconnu.
- `deliver_cancel` consomme la projection metadata du Writer ; `BatchExecutor::cancel` (`executor.rs:208`) vérifie son scope contre celui du Writer, construit uniquement les registrations des cibles prouvées et n’ouvre aucun provider blob/AcceptedJob/fresh start. Le reçu renvoyé conserve sa première révision.
- Les cibles sont tentées par groupes de 16, avec partage du budget restant de 45 s ; échec de construction/RPC d’une cible ne saute pas les autres. Le diagnostic incomplet est testé après les tentatives connues ; il refuse la complétion. Aucune complétion ne prouve un terminal runtime ni une libération de quota.
- Dans le port STOP, l’intention locale est marquée avant le premier RPC. Register est ObserveOnly et Cancel utilise le maximum de génération remote/local pour cette seule cible ; aucun advance_to partagé n’est exécuté dans cette branche. NotReady/perte de réponse n’empêche pas de tenter le Cancel par hash exact. Une divergence est Conflict et la cible inconnue n’est pas insérée après le fuse. La faille P1 concerne le choix du client préalable à ces contrôles.

**Limite de concurrence explicitée.** `restore_slot` peut choisir sa branche ordinaire avant un await Register, puis recevoir le marqueur retained pendant cet await et effectuer son `advance_to` ordinaire. Le port direct et la reprise avec marqueur déjà présent évitent le seed ; une garantie linéarisable « aucun seed dès qu’un marqueur arrive pendant une reprise » n’est pas démontrée. Un simple recheck atomique séparé ne prouverait pas cette garantie : il faudrait sérialiser les deux décisions. Le client failed a déjà fermé tous ses guards ; aucun effet supplémentaire indu sur un worker ultérieur n’a été établi. Ce risque est une limite de portée/concurrence, pas un second P1 démontré.

## Preuves et limites des harnais

Le test handler emploie un vrai Journal/Writer puis une couture de livraison explicitement bornée ; il démontre la redélivrance du replay et la stabilité du gate/head/génération, pas l’IPC ni un arrêt runtime. Les tests natifs utilisent un vrai child et Unix IPC authentifié, avec un programme runtime inexistant : leurs acteurs restent NotReady et aucun Docker/Podman n’est exécuté.

Le premier essai natif a échoué dans sa préparation : Register avec observe_only=false attend jusqu’à 44 s côté serveur, mais l’appel du harnais avait IO_TIMEOUT=3 s. Cette erreur de fixture ne démontre pas une régression produit. Dans le nouveau gel natif `3ec5fa8c…92feb`, la préparation conserve observe_only=false et borne délibérément la lecture à 100 ms : elle exige l’erreur précise Unavailable puis contrôle par Slots l’ID, le hash, la génération et prepared_once=false. Ce contrôle positif prouve que la réservation IPC a réussi malgré le retour perdu ; il ne confond pas toute erreur de transport avec NotReady. Les appels STOP ultérieurs utilisent leur Register ObserveOnly borné et peuvent ensuite envoyer Cancel à cet acteur indisponible. Le reviewer n’a modifié aucun de ces fichiers.

**NOT_RUN par cette revue :** toutes les commandes ; délais sous IO/CPU bloqués ; arrêt runtime réel ; crash/rotation d’enforcer complets ; autres UID/plateformes. Ne pas déduire une qualification Docker, un terminal worker, la capacité ou la fermeture de G09 depuis ces tests metadata/IPC.

## Gels et suivi

Gel initial : client `034194d1f928622c03cddd7f7f50ca3cf6567e3c926ae9744e8a900fbb0dc888` ; executor `7dcf635aacee5dd76465930ca63b8e3fc25b43545cfa28233785b433781d9514` ; server `732eb63174473cc9f846b10939e6d39bb68850b838a04756d41050ad1248497f` ; jobs `ad24a10c752043b0db772ba481664dfc44750b252966915b54e869192bb47aec`. Tests natifs `4eff5ffdc441357b8c0a8f9a6e6eab69a16d0db8cc00769122de2bdb4f3f9d3d` ; handler `36d0faf5449483daf333cd370f02776219fb35b6fa97c558e61ae4b5c18bfefb` ; checkpoint `e57c773b1ca19b08be9288fa100d68426c8e7a77e231deb26901020cb2ba6898` ; projection/doc EXACTJOB `2f6b66f66f7358377e86c774667359ac418658beba121a0e13e893dcd51a455e`.

Nouveau gel relu et hashes confirmés : client `8357172847801243049c1e00710edb0225951adc2b6d7fb149523caf03039834` ; natif `3ec5fa8c397c8595bb7a669e3c7c270f8af06837aa6e33592fa4f2c1b5392feb` ; executor `7dcf635aacee5dd76465930ca63b8e3fc25b43545cfa28233785b433781d9514` inchangé. Preuve Luna de la régression de chaîne à joindre par l’orchestrateur ; non exécutée ici. Les raccords de compilation import explicite LocalCommandRequestV1 et getter cancel.current() ont été signalés par Root ; ils ne changent pas ce constat fonctionnel.
