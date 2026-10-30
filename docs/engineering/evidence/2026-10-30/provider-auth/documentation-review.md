# Revue documentaire ciblée — état courant du 30 octobre 2026

**Verdict final : aucun constat ouvert dans cette revue ciblée après correction de DOC-01/02/03.** La revue ne qualifie aucun nouveau parcours natif ni la release.

## Périmètre et méthode

Lecture d'AGENTS.md et des documents root `docs/engineering/{DECISIONS,CURRENT_STATE,VALIDATION,IMPLEMENTATION_PLAN,PRODUCTION_READINESS}.md`, des dossiers de preuve `evidence/2026-10-30/{provider-auth,provider-cli,local-judge,jev-real}`, puis des quatre guides gelés par leur propriétaire : `PROVIDER_AUTH.md`, `docs/{quickstart,operations,api}.md`. Rapports image/Jev/local et résumés publics recoupés ; vérification de quelques branches du code lorsque nécessaires au contrat documentaire.

Aucun code/document du dépôt modifié par le relecteur. Aucun test produit, build, moteur, API, modèle ou nouvelle campagne globale exécuté. Aucune source réelle de credentials, aucun `.env`, cache OAuth ou keychain ouvert. Les liens externes cités par les auteurs n'ont pas été interrogés à nouveau ; cette revue recoupe leurs usages documentaires et les preuves locales, sans prétendre refaire leur vérification externe.

L'essai Claude Haiku 5.5 géré annoncé en cours n'est **pas** compté comme PASS. Selon le dernier message root reçu pendant cette lecture, un défaut de maintien du gateway avait empêché son premier essai avant HTTP/worker/API ; sa correction de harnais et tout résultat ultérieur nécessitent leur propre preuve. Rien n'est extrapolé à partir de cette information de progression.

## Constats

### DOC-01 — P2 documentaire, fermé pendant la revue : sondes bootstrap présentées comme dépourvues de credentials

Version initiale de `docs/engineering/PROVIDER_AUTH.md:143`, SHA `e887de735aa7f78be8b1f2cf7033f5fb0a04d8eeb001c3626a6c337ebe72fce3` : « Les sondes version/aides sont bornées, sans credential ».

Raisonnement reproductible par lecture : `src/provider/bootstrap.rs:126–128` installe l'`auth.json` Codex abonnement avant `probe()` ; `probe_environment` retire trois variables secrètes mais conserve `CODEX_HOME` (`:345–355`), qui pointe vers ce fichier. L'ordre `prepare_execution` puis `probe` est visible à `:525–531`. La sonde peut donc lire le credential abonnement ; l'absence des variables ne signifie pas absence de tout matériel d'authentification. Cela ne contredit pas le PASS des sondes image sans handoff, qui sont un autre cas réellement sans credential.

Root a corrigé le paragraphe : variables d'environnement expurgées, fichier abonnement déjà présent et accessible, distinction des sondes image sans handoff. **Correction relue et acceptée**, nouveau SHA `b104f8346c19c303cb791d5185bb9ad8edbcef047d7c8653d2a12113ca0889f7`. Aucune mutation de code ni nouvelle inférence nécessaire. Aucun abus effectif du fichier par une sonde n'est affirmé.

### DOC-02 — P3, précision du refus d'invocation bootstrap, fermée pendant la revue

`docs/api.md:111` dit : « Une invocation directe du programme bootstrap sans résolution gérée est également refusée. » `src/policy.rs:145–149` compare précisément `argv[0]` au chemin canonique `BOOTSTRAP_PROGRAM`. Ce test n'atteste pas l'identité de tous les chemins/alias possibles vers le même exécutable.

Recommandation transmise à root : nommer explicitement l'invocation au chemin canonique `/usr/local/bin/ai-guard-provider`. Il s'agit de borner la phrase à la garantie livrée, sans assimiler le parcours manuel avancé à une nouvelle résolution gérée. Aucun nouveau contournement d'autorisation ou lancement non autorisé n'est démontré par cette observation documentaire ; aucun test d'exploitation demandé.

Root a corrigé la phrase en précisant `argv[0]`, le chemin canonique et l'absence d'attestation des autres alias/programmes transitifs. Correction relue contre `src/policy.rs:145–149`, sans nouveau test : **fermé**, SHA API final `0cdc3c4d4334196fbd9c13fa683d9761739a155829ecc5789b46b09b6d1d285a`. Les hashes runner/bootstrap/policy recoupés restent ceux des gels de code précédents ; aucun changement de code attribué à ces corrections documentaires.

### DOC-03 — formulation chronologique Jev, fermée pendant la revue

`docs/engineering/DECISIONS.md:7` indiquait encore qu'un unique appel était « prévu », alors que CURRENT_STATE, VALIDATION et le dossier Jev consignaient son achèvement. Root a remplacé cette formulation par le résultat daté et un lien vers les preuves. Lecture de correction effectuée : aucune nouvelle promesse sémantique ajoutée, ancien projet et résultat distingués.

## Points de qualification recoupés

- **Modèle local :** `local-judge/README.md`, `real-trial.md` et CURRENT_STATE/VALIDATION séparent mémoire/protocole PASS de sémantique FAIL. Le pic cgroup de 777 912 320 octets est inférieur au plafond effectif 999 997 440, mais ne qualifie pas 500 Mo ni d'autres charges/plateformes. La réponse `allow` à la suppression synthétique est bien présente dans `request-2-output.txt` ; aucune commande destructive n'a été exécutée. Le schéma exige une forme, pas ce verdict. Deux requêtes aux prompts différents ne sont pas vendues comme benchmark contrôlé ; le candidat est refusé comme autorité d'exécution. Le préflight NOT_RUN reste historique et un lien conduit à l'addendum final.
- **Jev réel :** le dossier décrit un seul POST externe ; le timer hors runtime et la demande refusée par l'allowlist locale sont des prévols sans appel Jev. Réponse publique et résumé d'audit concordent : `jev-1.14.0`, `allow`, confiance déclarée 1,0, usage déclaré 475/39, séquence 3 et même tête/hash. La réouverture d'audit n'est pas un second appel. Aucune disponibilité, calibration, facture ou qualité générale n'est déduite. Le binaire `bb48545a…8047` reste antérieur au garde Jev du handoff.
- **Fournisseurs :** les anciens PASS batch manuels Codex/Claude et leurs artefacts restent distincts des plans gérés/bootstrap nouveaux. L'ancien alias `haiku` n'est pas requalifié Haiku 5.5. Les FAIL TUI liés à la collecte/instrumentation restent des FAIL de qualification ; les diagnostics locaux sans modèle ne deviennent pas deux tours API.
- **Image :** build/versions/aides/refus sans handoff PASS sont présentés comme tels. Les inspections publiques réduites montrent les scalaires de confinement attendus, pas un dump d'Env. Le postinstall Claude non autorisé reste signalé au gel de cette preuve. Aucune authentification réussie n'est déduite des aides ou d'un code 126 attendu sans handoff.
- **Logiciel :** 388 PASS/8 ignorés avant revue et 25 PASS/1 fixture ignorée après correction ne sont ni additionnés ni transférés au dernier delta runner. Les PRV-01/02/03 fermés ont leur gel propre. Le test Jev handoff est rapporté avec historique outil seulement ; aucune archive stdout reconstruite n'est revendiquée. Le manifest avant campagne post-revue absent reste déclaré.
- **Parcours :** `providers status` décrit la configuration et conserve les deux indicateurs de vérification à false ; `doctor providers` et assistant login/import/status ne sont pas présentés comme livrés. Observation d'un client, auth et conversation restent séparées. Les profils abonnement, routes/settings adverses et tokens rafraîchis restent ouverts. Claims consommés, refus de réattachement et limites de supervision autonome sont conservés.
- **Gates :** 0/12 entièrement clôturés ; l'estimation de développement est explicitement une estimation d'ingénierie, pas un score de qualification. Le PASS RAM ne ferme aucun gate décisionnel. Le flood du harnais déjà corrigé n'est pas rouvert ; la réserve distincte des awaits d'ACK du runtime subsiste.

## Liens et confidentialité — portée exacte des contrôles

L'outil Rust déjà compilé `/tmp/ai-guard-engine-docs-scan/links` a été utilisé après lecture de `links.rs`, sur les dix Markdown racine/guides cités et les quatre dossiers de preuve uniquement. Résultat constaté : **36 fichiers Markdown, 271 cibles locales vérifiées, 0 cible absente, exit 0**. Ce parseur simple ignore les fragments, HTTP(S), mailto et les ancres seules ; il ne vérifie pas HTML ou résolution des URLs externes. Il ne constitue pas une preuve globale de tous les liens du dépôt. Les principales ancres nouvelles PROVIDER_AUTH, Jev optionnel et addendum local ont été recoupées par lecture de leurs titres.

La lecture des résumés publics et une recherche heuristique de formes de clés/Bearer/affectations longues dans le périmètre root et les quatre dossiers n'ont signalé aucune valeur secrète reconnaissable. Aucun dictionnaire de secrets réels n'a été lu, donc **aucun scan littéral exhaustif indépendant n'est revendiqué**. Les anciens relevés root « 523 fichiers / 0 occurrence » et « 196 liens / 0 absent » ne sont pas réutilisés comme preuve des ajouts ultérieurs. Les identifiants/hash publics, chemins de preuve et nombres d'usage ne sont pas confondus avec des credentials.

Empreintes finales des guides relus, incluant les corrections de cette revue :

```text
b104f8346c19c303cb791d5185bb9ad8edbcef047d7c8653d2a12113ca0889f7  docs/engineering/PROVIDER_AUTH.md
8a56d638b6a64c08786eca188a876e724a56e9348784ec889d0f82c1d292fcd6  docs/quickstart.md
0bce85aad0ee415cbfca43b8584404719292fb1d34747ec6b53c7c4015147d2d  docs/operations.md
0cdc3c4d4334196fbd9c13fa683d9761739a155829ecc5789b46b09b6d1d285a  docs/api.md
```

Les résultats futurs de l'essai géré en cours et leurs changements documentaires restent hors de ce verdict jusqu'à recoupement de leur rapport final. Les quatre parcours ne sont pas déclarés qualifiés par cette revue.

## Addendum final — essai Claude/API géré et artefacts reconstruits

Le rapport final est maintenant disponible : `/tmp/ai-guard-managed-claude-api-real.md`, identique à sa copie publique `docs/engineering/evidence/2026-10-30/provider-auth/managed-claude/real-run.md` (comparaison `cmp` effectuée, exit 0). Cet addendum complète le verdict pré-essai ci-dessus ; il ne réécrit pas ses statuts historiques.

**Verdict : PASS ciblé Claude/API batch géré cohérent avec les preuves relues, sans nouveau défaut documentaire bloquant confirmé.** Il porte sur une invocation native du client après les prévols, pas sur un nombre de requêtes HTTP fournisseur mesuré. Le modèle demandé et résolu localement est `claude-haiku-5-5` ; le modèle effectivement servi reste inconnu. Codex géré, les abonnements, TUI, réglages adverses et les autres plateformes ne deviennent pas qualifiés par ce résultat.

### Recoupements effectués sans exécution produit

- L'artefact public `managed-claude/result/result.txt` fait **42 octets**. Comparaison exacte avec le canari ASCII attendu, sans newline : exit 0. SHA recalculé `cc2b93ab193564857b7236f4d1569949d368fc55374dd6976daad6fa334fb6b0`. Le fichier public `task.txt` a le SHA attendu `df490e68ccded9f6314ec2ba88e0e305117eec55e5fdb713021ef6363c6a1c83`. L'absence de modification de l'original et l'identité de l'entrée exportée sont établies par le rapport et le recoupement root ; le relecteur n'a pas ouvert le projet privé ni les bindings.
- `verify-audit-reopened.json` donne `valid=true`, 36 événements et tête `31274bcc970ee1c7c9beacc4dc7bdfda367c4c655b2cb9c9c22b3ba5bf4e24fa`. `usage-audit-probe.json` place l'admission à 4, le claim à 13, l'observation terminale à 16 avec quota libéré, puis cleanup et export. L'allocation présentée porte `provenance=observed`, `billable=false`, durée `5923182878` ns. Le rapport atteste une unique allocation ; cette revue n'a pas réinterrogé le registre ni la base. Ces chiffres ne sont pas du CPU consommé ou une facture.
- Les deux échecs de prévol restent explicitement conservés : gateway arrêté avec sa session d'outil, puis absence de l'exception rootful Desktop. Ils précèdent tout worker/modèle et ne sont pas réétiquetés échecs Anthropic. Le PASS vient de la tentative fraîche suivante, sans réemploi de claim. Le nombre d'appels internes du client au fournisseur demeure inconnu ; « une invocation CLI » ne devient pas « un POST externe » comme dans le cas Jev distinct.
- Le warning npm postinstall n'a pas empêché ce batch réel ; le texte n'en déduit pas le bon fonctionnement de tout l'installateur ou de tous les parcours. Le preset n'a pas de plafond monétaire ; les 75 secondes configurées ne sont pas un budget API. Le bridge ne devient pas un filtrage egress ni le prompt une médiation transitive.
- La preuve native reste liée au binaire gelé `bb48545a6c7f6af0ab9b2b1e4b509927682ae6354d7372a979df1135d4058047` et à l'image worker `069ba7b703077238f22edbefc9295a11cc07e71e8dd490b8de7ab81da51ffa38`. Le hash de la copie binaire préservée a été recalculé sans exécution et concorde.
- `final-artifacts/validation.md` distingue désormais correctement le garde Jev `runner.rs:923` du garde NoLogs `control.rs:436`, ainsi que les hashes source/exécutable. Les deux manifests de **89 fichiers** sont identiques (`cmp`, exit 0). Les logs Clippy et build se terminent avec succès ; fmt exit 0 est rapporté avec son log vide. Aucun test ni appel natif n'a été relancé pour ce rebuild. Hash du binaire reconstruit recalculé : `ce8e94ff4ba6437799101bf8b093b7737fb8c89ff778591b35a4f20bba655917`. Hash source runner recalculé : `d8ee798b3df2a942e514b79b17eb1937cf812c7dbad01a478eae08b123082ea4`. **Aucun transfert rétroactif du PASS natif au nouveau binaire.** La comparaison source/binaire erronée du brouillon est retirée et explicitement conservée comme erreur documentaire, pas comme échec du build.

### Documentation et confidentialité

Derniers paragraphes CURRENT_STATE, VALIDATION, PROVIDER_AUTH, API, quickstart, opérations, README provider-auth, plan et gate G07 relus : le PASS géré est ajouté sans convertir les anciens essais manuels ni les TUI en nouvelles qualifications. La proposition historique et le gel build/probes conservent leurs anciens NOT_RUN ; les paragraphes récents portent le résultat suivant. Une formulation mineure de CURRENT_STATE:19 « les quatre chemins restent non qualifiés » a été signalée pour préciser son rattachement au gel des sondes, puisque le PASS ciblé suivant est désormais présent à :23. Elle n'est pas interprétée comme annulation du nouveau PASS ni comme qualification complète des quatre chemins.

Root a ensuite remplacé cette formulation de CURRENT_STATE:19 par « les autres chemins gérés » et précisé que le warning postinstall n'a pas empêché le batch ultérieur. Correction relue : **ambiguïté fermée**, aucun constat documentaire ouvert dans cet addendum.

Le relevé de confidentialité root est **rapporté**, pas réexécuté par cette revue : 39 fichiers logs/projet/export/rapport, cinq valeurs littérales connues (trois clés et deux tokens du contrôle dédié), zéro occurrence. Les bindings privés intentionnellement porteurs du secret sont exclus et aucune garantie sur tous les encodages n'est annoncée. Les preuves publiques relues sont des résultats synthétiques et résumés scalaires ; aucun dump de credentials, Env ou base privée n'a été ouvert/copié par le relecteur.

Nouveau contrôle de liens via l'outil Rust existant, sur les huit documents actualisés et provider-auth seulement : **23 Markdown, 244 cibles locales, 0 absente, exit 0**. Même limite que précédemment : pas de contrôle des fragments/HTML/URLs externes, ni campagne globale de liens. Les anciens nombres de contrôle ne sont pas réutilisés pour ces ajouts.

Empreintes des preuves effectivement relues :

```text
ebc753f53a55126d9e9c1a2a399e95fcb1e05ac032a5562c3db77e1c4e1ef4f0  managed-claude/real-run.md
507bd4af21991d65e55b9953f9ccc0bbcddb2dcaf164a5fb9ccf53693febcbde  managed-claude/usage-audit-probe.json
afadb61015418f77f74c5b94ff0ff03531f1d7249323a63e27cd8461c4537939  managed-claude/verify-audit-reopened.json
de82d3fb50ec48429b303bfdd45d3b63e0bb38b0764c572d35eedf11af9a0502  final-artifacts/validation.md
```

Toutes ces entrées sont sous `docs/engineering/evidence/2026-10-30/provider-auth/`. Aucune nouvelle qualification de release, aucun gate entièrement fermé et aucune exécution produit par cette revue. Les réserves OAuth/abonnements, modèle servi, routes adverses et supervision autonome restent ouvertes.
