# Risk coverage review — intake and follow-up, 12 November 2026

## Authority and reading order / Portée et lecture

This register preserves the independent source risk analysis `ai-guard-risk-coverage-2026-11-12`. It is a mandatory input to later confinement, harness, identity, runtime, audit and deployment work, alongside [run-1](AUDIT_FOLLOWUP.md#audit-final-run-1) and [run-2](AUDIT_RUN_2_20261110.md). It supersedes neither report. Read this register, then the original `REPORT.md`, `CODEX_HANDOFF.md` and relevant entries of `RISK_REGISTER.json` before a correction. The annexes precede the report’s adjudication; the final structured register and report govern their disagreements.

Ce suivi conserve l’analyse indépendante de couverture des risques. Les priorités P0–P3 sont celles du rapport ; elles ne constituent pas des sévérités de vulnérabilités reproduites. Aucun build, test, moteur, fournisseur, navigateur ni réseau cible n’a été exécuté par cette analyse : tous ses tests sont **NOT_RUN**. Les garanties relues sont des constats de source avec réserves, pas une qualification runtime. Les résultats historiques cités ne sont pas transférés.

## Identités et conservation

| Élément | Identité vérifiée |
|---|---|
| Source principale examinée | `711ba1378672f5c66099b2dd78daee2b5e35019f`, tree `8c282a50f797ec8ba432377bc61dbb31674f536e` |
| Delta E examiné séparément | `cf272b20ff126d75074d7347852c51454df58562`, source validée `469d6259c305c415695968334bf3a1eb5b355a5c` ; sa composition privée était non examinée |
| Comparaison initiale des fichiers | public `6699a13330080263c3588f17f1dd006b290710de`, tree `e8a96023044de487fcb69594aaa20a87faaf8e20` ; source auditée ancêtre vérifié |
| Pièces privées | `.claude/audit/ai-guard-risk-coverage-2026-11-12/report/`, ignoré par Git |
| Intégrité | 34 fichiers réguliers, 3 988 823 octets ; les 33 empreintes de `SHA256SUMS` correspondent ; copie byte-identique, originaux conservés |
| SHA-256 du manifeste | `0303624121a836083beac6e67cb5e418c1286b6b871e7358333f8be3b0d849a6` |
| SHA-256 REPORT.md | `0d5cea88891e69c583fb12d5b9c2a6e944688e723af90ab412eab15a73fac1e1` |
| SHA-256 RISK_REGISTER.json | `dd2e4e3d96c9b0a91fd3a21eb3886f8e4a09c8d2050a2dd71613ec33a28264e3` |

L’archive privée contient les neuf livrables et `annexes/`. L’accusé local `INTAKE.json` conserve la provenance et la vérification de copie sans modifier les pièces. Les originaux restent dans le dossier de livraison de l’analyse ; leur existence et leur manifeste sont revérifiés à la réception. Les rapports privés ne sont pas publiés avec ce suivi. Avant fermeture du worktree qui porte la copie privée, conserver celle-ci et les accusés hors du checkout puis vérifier leur manifeste, selon le cycle de vie d’[AGENTS.md](../../AGENTS.md).

Le message de livraison citait aussi la tête intermédiaire `3cc7694`, non examinée par l’auteur. La comparaison initiale ici porte explicitement sur `6699a13` ; les changements privés non committés sont exclus. Les lots Claude A–E ont depuis leurs propres intégrations et preuves dans [leur registre](CLAUDE_DELIVERIES_20261110.md), sans qualification rétroactive par ce rapport.

## Inventaire conservé

| Catégorie du rapport | Nombre | État à la réception |
|---|---:|---|
| Entrées retenues | 80 | À rapprocher individuellement du code et des conditions actuels |
| Priorités retenues | 1 P0 conditionnel, 19 P1, 45 P2, 15 P3 | Priorités source, pas confirmation dynamique |
| Hypothèses rejetées | 11 | Rejets historiques conservés, pas étendus à une autre source |
| Entrées fusionnées | 3 | Liens de fusion conservés pour éviter les doublons |
| Hypothèse rouverte | 1 | RC-P6-R14 rattachée à RC-P6-R04 ; expérience requise avant promotion |
| Garanties | 93, dont 39 relues | Aucune requalification actuelle déduite |
| Scénarios | 70 | 6 empêchés avec réserve, 17 non couverts, autres états conservés ; tests NOT_RUN |
| Couverture et rapprochements | 123 lignes, 124 rapprochements antérieurs | Portée exacte du rapport seulement |
| Non examiné | 140 éléments | Ni couverts ni réputés sûrs |

Le [registre de suivi typé](RISK_COVERAGE_20261112.json) conserve les 95 identifiants/fingerprints, statut et priorité du rapport, lot, liens de fusion, puis un **état courant distinct**. Sa comparaison de blobs compte 269 références inchangées, 84 modifiées et 2 non résolues ; un fichier inchangé ne prouve pas que toutes les conditions restent identiques et un fichier modifié ne prouve pas un correctif. Les références `E:` sont rapprochées du delta E, non de la source principale. Les références larges `src/` restent explicitement non résolues. Aucune entrée n’est clôturée à la réception.

## Priorités concrètes et travaux indépendants

Les conclusions à traiter en premier concernent la médiation des sorties réseau, la clé fournisseur lisible dans le worker, les outils Web/MCP distants hors confinement local, le contenu actif au retour de l’export, l’arrêt autonome et l’attribution nominative des approbations. Ces sujets doivent être vérifiés séparément : une décision de lancement ne constitue pas une décision par outil ; une instruction humaine déclarée ne prouve pas un approbateur ; une inspection déclarée par le moteur ne démontre pas tous les effets du confinement.

Le rapport propose onze entrées de vague 0 sans décision structurelle nouvelle :

- RC-P5-R01
- RC-P6-R06
- RC-P6-R03
- RC-P1-R01
- RC-P1-R02
- RC-P2-R07
- RC-P4-R02
- RC-P3-R10
- RC-P3-R07
- RC-P3-R12
- RC-P7-R10

Avant édition, comparer leur fingerprint, leurs symboles et leurs conditions au gel courant, lire leur fiche `ACC-…`, puis attribuer des propriétaires de fichiers et des worktrees disjoints. Réutiliser un chantier en cours lorsqu’il traite réellement le même défaut ; un helper voisin ou un PASS historique ne suffit pas à clore le résidu. La revue et les tests concernés restent requis. La triage initiale est conservée localement ; tant qu’elle n’est pas validée, ces onze entrées restent à revalider dans le registre public.

## Cinq décisions à rapprocher avant changement structurel

| Référence | Question du rapport | État |
|---|---|---|
| RISK-D1 / LOT-A | Architecture egress : réseau possédé et relais Rust, ou transport exec avec réseau none | Rapprocher les décisions existantes, préparer une proposition concrète et la soumettre si une nouvelle frontière est nécessaire |
| RISK-D2 / LOT-D | Politique d’organisation appliquée à l’admission, profil déclaratif commun au build et à l’exécution | Même règle ; aucune décision de format/emplacement présumée |
| RISK-D3 / LOT-B | Garde de la clé fournisseur et credentials d’outils pour destinations nommées | Limite actuelle à exposer ; aucun relais d’inférence ni accès externe implicite |
| RISK-D4 / LOT-C | Traitement par défaut des fichiers de pilotage des harness à l’import et à l’export | Pas de quarantaine destructive ou suppression de fichiers utilisateur sans décision applicable |
| RISK-D5 / LOT-E | Qui porte l’arrêt autonome et son échéance | La direction broker permanent déjà autorisée ne prouve pas une couverture de tous les parcours |

Ces questions ne constituent ni autorisation nouvelle, ni acceptation de risque, ni blocage du travail indépendant. Les préférences de contexte défaut/homelab/entreprise doivent rester réglables dans les limites centrales ; enlever un client SSH de l’image ne suffit pas à imposer une politique réseau.

## Règle de clôture et continuité

Pour chaque entrée : noter la source exacte et les conditions actuelles ; reproduire lorsqu’un oracle local autorisé est disponible ; conserver le correctif et sa régression ; faire relire indépendamment ; vérifier le résultat sur le gel intégré ; synchroniser les guides EN/FR et les profils concernés. Renseigner le propriétaire, les preuves, limites et statut actuel dans le registre. Une qualification Docker ne remplace ni Podman ni Kubernetes ; une fixture logicielle ne remplace aucun moteur réel. Aucun constat des audits précédents ni gate G01–G12 n’est clos par ce seul archivage.

La limite de quota ayant interrompu deux agents Luna à la réception est un incident d’outillage distinct, sans essai cible : leurs QA préparées restent NOT_RUN. Elle ne transforme pas un risque en échec ou réussite du produit. Les travaux de lecture, de documentation et les lots indépendants continuent.
