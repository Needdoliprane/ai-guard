# Revue indépendante — préparation broker et transport préarmé

31 octobre 2026. Worktree exclusif `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, base `7098e204a91182229c1a67bc8017271cb3d20918`. Lecture de code, contrats, oracles et logs ; aucun Cargo, test, moteur, modèle ou changement du dépôt par le reviewer. Les rapports détaillés privés sont `/tmp/ai-guard-broker-submission-review.md` et `/tmp/ai-guard-broker-runtime-review.md`.

## Périmètre et gels

| Source | SHA256 |
|---|---|
| src/runner.rs | ba72d730ee8d611196bf1130ab8bfc9bd56ad1d3b7f9ea5996b1221daa5efac2 |
| src/runner/broker.rs | 83bc1e6ab623bc6cc2b6314f0e0b32c4007b0c4c1bda02a0e693f297d1a8930b |
| src/runtime.rs GEL2 | 8d63a1caaf3f3b2ef186813856f9213be029a8662fdb911552b96f5546500d22 |
| src/runtime/managed.rs | a2a5be94e7cfec58b440195805ab006c80d50807a273e100b6d03c53265b703e |
| tests/broker_submission.rs final lu | 1b5b7b31d4bb37bc0de5c9ae272b7cd076d7b7cfda546d07a6c76c2a4579954f |
| tests/runtime_prearmed.rs final lu | ac8901fcaac3d9944daaa40bebc41d86825dd1f12332b64584821c049c84a24f |

SHA des six fichiers recoupés dans le worktree. Le contrat actif est la section du cycle permanent/batch dans IMPLEMENTATION_PLAN et le contrat privé de soumission ; ces ports ne sont pas le service complet.

## Conclusion source

Préparation : aucun défaut bloquant identifié. UUID imposé avant projection ; nil, approve_hash, batch interactif et bornes des littéraux refusés avant choix/IO. Erreurs publiques opaques. Freeze fournisseur conserve le PrivateFile déjà écrit/synchronisé au lieu de relire un chemin libre. Le chemin broker fournit les littéraux de contrôle sans relire les clés de contrôle legacy. Reconstruction BrokerRunState pure depuis intention et checkpoint, sans Auto, état d'approbation réinitialisé ou lookup original. Handoff depuis le seul binding durable retenu, type/identité/digest/names/handoffsha recoupés ; FD et Directory sont conservés. Validation du binding utilise persistent_revalidate avec FileExt/read_at et métadonnées, sans second FD source. Aucune garantie rétroactive sur une ouverture caller antérieure ni sur un acteur root/même UID n'est inventée.

Runtime : RPT-01 était confirmé au GEL1. Un callback déjà commencé pouvait être annulé à EOF puis reconstruit, produisant deux opérations possibles malgré une hypothèse d'idempotence du Store. RED Luna archivé dans `/tmp/ai-guard-prearmed-gel1-callback-qa/eof-red.log` : RC101, compteur2 contre1 attendu. Le correctif GEL2 conserve exactement le même futur épinglé pendant EOF/inspection terminale/attente callback ; aucun second appel dans finish. Stop sélectionné prioritairement pendant ces attentes et les captures. Absence StartedAt et terminal incohérent refusés. RPT-01 fermé en source, aucun autre écart bloquant identifié.

Le garde consommable est adjacent à spawn après vérification d'autorité et préparation Command, sans await/IO/log intermédiaire. Le nouveau transport refuse TUI avant create et n'ajoute ni Ctrl-C global ni durée métier. Résultat public sans timed_out. Après stop, drainage du seul client borné10s puis erreur/retention si inconnu ; le chemin d'erreur préarmé ne bloque pas sur un stop inline. Identité/autorité/configuration/allocation et terminal strict conservés. La factorisation legacy conserve son comportement propre.

## Oracles finaux relus

Soumission5 fonctions : vrai Journal de fichiers avec binding None et approbation V1 conservée, erreurs précoces opaques, borne littéraux, manuel et fournisseur. Le test d'approbation est une reconstruction V1 synthétique, pas une preuve de permission ni du futur cycle d'approbation V3. Les refus précoces vérifient l'absence de création de chemins ; l'absence de toute lecture est établie par l'ordre source, pas par cette seule assertion.

Manuel : source renommée/remplacée après accept, octets exacts du Snapshot comparés, SHA256 égal au manifeste, noms et absence de la valeur tournée, Debug opaque. Validation après Drop(job, prepared), puis modification du Snapshot et refus.

Fournisseur : mêmes contrôles bytes/SHA/noms/rotation. Suppression de providers.json et de tout credential-bindings avant nouveau load/reconstruction ; pas uniquement rotation de provider.env. La construction rejoue les octets initiaux, puis valide après Drop des owners et refuse le Snapshot altéré. Les canaris sont synthétiques ; aucun fournisseur n'est appelé.

Runtime4 fonctions : deux tests de forme/type/debug et deux scénarios asynchrones isolés dans des enfants. Fixture CLI Rust compilée, pas moteur. Refus interactif avant création ; ObserveOnly sans start ; reprise d'un Created uniquement avec nouveau contrôle ; signal true/canal fermé/garde refusé sans émission, compte start relatif au nombre avant chaque scénario. Callback pending n'absorbe ni stop indépendant ni EOF ; compteur d'entrée à1 après EOF et reprise Notify. Les captures exactes et terminal synthétique sont recoupés. SIGINT est émis seulement dans un enfant avec handler service distinct : ce test ne signale pas le processus parent. Sous-cas et exécutions enfant ne s'ajoutent pas aux quatre tests comptés.

## Preuves finales lues

Logs complets lus dans `/tmp/ai-guard-broker-service-cycle-final/` : broker-submission-attempt3=5/5 PASS RC0 ; runtime-prearmed-final=4/4 PASS RC0 (13,94s) ; canary-credential-handoff=1/1 PASS (6filtered), RC0 ; canary-runtime-managed=14/14 PASS RC0 (25,90s) ; fmt-final RC0 sur les six SHA finaux inchangés ; Clippy final RC0. Total ciblé :9 fonctions nouvelles et15 canaris, pas addition des sous-cas/enfants. Build release PASS RC0 (38,14s), log complet lu. Rapport propriétaire corrigé `/tmp/ai-guard-broker-service-cycle-final/summary.md`, SHA256 `c172cb83cfd9877ae41c6b96e84bc7cc94b16320cb1b3943ccccbdbddf36842d`, relu : il décrit désormais le Journal à fichiers privés sans Store SQLite et le SIGINT réellement envoyé à l’enfant de test, distinct de l’état moteur simulé. Les copies final/ des deux tests ont exactement les SHA du tableau. Fmt et Clippy sont des contrôles statiques, pas des fonctions de test supplémentaires.

Historique conservé : une tentative de harnais fournisseur échoue après rotation à cause d'un chemin macOS non canonicalisé (attempt2 RC101), puis correction du test sans delta app et cible complète PASS. Clippy initial RC101 sur un if imbriqué du test EOF, corrigé par une let-chain ; diff exact relu, aucune modification de l'oracle. Test runtime précédent6622cdb6 archivé ; finalac8901fc recoupé dans worktree et copie final/, cible entière4/4 rejouée ensuite. Les RED API absentes sont des refus de compilation, distincts du RED comportemental RPT-01. Aucun échec historique n'est effacé ou requalifié en succès.

Verdict final de cette revue : **CLOSE au périmètre logiciel, documents, preuves et artefacts des deux ports**. RPT-01 corrigé et régression ciblée GREEN ; aucun défaut app ou oracle bloquant résiduel identifié dans les fichiers lus. Les quatre sources applicatives sont demeurées aux gels déclarés pendant le recoupement. Les archives du RED lient le test372f0a58, le binaire16eca8ad et les sources GEL1 conservées ; SHA de ces objets recoupés.

La release a été construite par Luna, pas par le reviewer. Le recoupement ultérieur des artefacts finaux hors target, du manifeste et des documents figure dans la section de clôture ci-dessous. Les limites demeurent impératives.

## Limites impératives

Ports Rust de confiance, pas authentification, Grant ou autorité centrale. Enforcer/horloge concrets indépendants, writer/service/IPC/CLI, cycle de vie local, raccord helpers et batch complet restent à implémenter/qualifier ; aucun cutover CLI permis par ce lot seul. D5 reste ouvert. Pas de qualification Docker/Podman réelle, autres plateformes, abonnement/API, TUI, arrêt à l'échéance après mort CLI, service OS, cloud/PKI ou production. Le contrat lifecycle V3 proposé est examiné séparément et ne devient pas livré par ce rapport. QA, G09 et gates produit ne changent pas par ces PASS logiciels.

## Revue documentaire et artefacts — gel du 31 octobre 2026

Relus : delta CURRENT_STATE/IMPLEMENTATION_PLAN/TEST_STRATEGY/VALIDATION et dossier evidence/2026-10-31/broker-submission-prearmed. Commandes, neuf fonctions nouvelles/cinq+quatre, quinze canaris1+14, zéro ignored aux deux cibles, six filtered au canari handoff, chronologie GEL1/GEL2/let-chain test finale et limites macOS/CLI Rust concordent avec les logs. Les RED de compilation, RED produit et erreurs de harnais demeurent distincts. L'absence de log brut DirectoryNotEmpty est honnêtement publiée. Aucun service/enforcer/gardien/D5/G09 n'est fermé et aucun moteur réel revendiqué.

Dossier privé `/tmp/ai-guard-broker-submission-qualified.oCJSTs` mode0700 ; manifeste, tar et quatre artefacts0600 constatés. Empreintes calculées en lecture : manifeste `84bcbb15d0e440975ee345c49c3530519421fb5c53abac865f3238139e897516`, tar `1e0b5b9944274fd0fb83d33b5cb0945811f1c1d887081c7aab5920ba4ccce7eb`, binaire `c80bf668f142ca0184ef947a77d7c3344ebef7e3cefd33e510d9dd9558129278`, lib `5e44d0295282394bee2f5183c6f982680f858d8c83d8c82e37d0c2bdb2bcc3a4`, test submission `1bddf1c997eb8353e42825dab12354c5962c0a0e416b90c642934192ada5f951`, test runtime `3ab7052fb33b7970f1676d7484024dd7949be7ff94d1d7ed88ca6ddb03832ea2`. Toutes égales au README.

Les118 entrées du manifeste correspondent byte-for-byte au checkout, au recheck extrait par root et aux membres source du tar. Aucun mismatch, aucun lien ou type spécial dans le tar. Le tar BSD contient aussi118 entrées AppleDouble `._*` de métadonnées macOS :118 désigne ici les sources logiques du manifeste, pas le nombre total de membres réguliers du tar. Cette présence privée ne change pas les bytes qualifiés. Manifeste copié dans le dossier repo identique. Les deux logs de vérification contiennent118 OK chacun.

Dix logs finaux/RED/archives et le rapport tests.md sont byte-for-byte identiques aux originaux privés lus. Test RED372f0a58 et fixture d035ff6c publiés en .rs.txt correspondent aux originaux archivés. Patch RPT-01 relu : seul delta runtime GEL1→GEL2, cohérent avec l'instruction d'inversion en copie isolée. Aucune inversion, extraction, build ou nouvelle exécution produit par le reviewer.

Les deux précisions rédactionnelles demandées sont appliquées et relues : README restreint l'interruption aux observations/on_started/finish après obtention du contrôle, explicite on_created/on_ready non interrompus par ce canal et distingue drainage10s du client attach et retour global de méthode ; PLAN désigne le garde concret horloge comme exigence future non livrée. README précise les AppleDouble en plus des118 sources logiques. Aucun écart documentaire bloquant résiduel.

Provenance fmt clarifiée : l'ancien fmt-check précédait la let-chain du seul test. Luna a exécuté un nouveau `cargo fmt --all -- --check` sur le gel final, RC0, six SHA inchangés. Le nouveau log est recopié byte-for-byte dans logs/fmt.log, SHA256 `7e1cd69fab85528649b3c67810caed1f9db026f90623c7d226ea1477c93b6a46` ; tests.md correspond au rapport propriétaire courant `c172cb83cfd9877ae41c6b96e84bc7cc94b16320cb1b3943ccccbdbddf36842d`. Aucun nouveau test, Clippy ou build nécessaire à cette clarification et aucun Cargo par le reviewer.

Clôture finale : aucune réserve bloquante source/oracle/doc/preuve/artefact restante pour ce lot. Le manifeste SHA256SUMS des pièces de preuve sera généré par root après copie de ce rapport ; sa génération, la copie du verdict et les scans prépublication ne changent pas le gel applicatif. Cette clôture permet l'intégration de ces composants, sans qualifier le service permanent ni réduire les exigences G01–G12.
