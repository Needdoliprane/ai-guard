# Supervision unifiée — composition du lot B

Candidate composée le 10 novembre 2026, sans qualification production. **0/12 gates clos** ; G08 et G10 restent ouverts. Les preuves de la branche Claude ne sont pas transférées à cette composition.

## Sources et périmètre

- Base d'intégration : `2cc29ee42ae3eb3cc8203538bf7a09aee2e26b57`.
- Source Claude : `claude/unified-control-ui-20261110`, tête `6b7aea7e4f34c5eb1ec351343095e30309e97947`, commit logiciel validé historiquement `2d65beaa070cb2258576bd4b5eb6e5f7950ce872`, base `7672078831c74047b98ce85babf91adb625e2b29`.
- Travail isolé : `codex/integrate-unified-ui-20261110`. La branche source, le checkout principal et le worktree de contrôle ne sont pas modifiés.
- Le handoff source de 1 528 lignes, sa proposition de contrat API et les journaux des deux écarts résiduels ont été relus. Le dossier historique reste disponible dans l'histoire source : `git show 6b7aea7:docs/engineering/evidence/2026-11-10/unified-control-ui/README.md`. Il décrit sa propre QA, pas celle de cette composition.
- Les 38 fichiers logiciels/documentaires du lot sont repris, avec deux correctifs frontend et les précisions documentaires ci-dessous. S'ajoutent le contrat dans [docs/api.md](../../../../api.md) et ce registre. Les manifestes complets, préimages, postimages et patches bornés sont conservés dans la livraison privée `claude-B-integration-20261110` du coordinateur.

Le seul raccord Rust aux fichiers partagés reste celui de `d0923de` : `mod recent` dans `src/audit/human.rs`, `mod recent` et une route dans `src/api/human.rs`, un chemin exact dans la liste du proxy de `src/api.rs`. Ces quatre lignes sont appliquées sur les fichiers de `2cc29ee`, sans remplacer leur contenu par celui de la base Claude. Le schéma d'audit 12, le partage des contextes par tenant configuré, l'identité partielle, les capacités d'authentification et les contrôles de révocation existants sont conservés.

La lecture récente conserve les prédicats de la lecture avant : organisation et tenant issus du contrôle, ressource active, permissions avant `LIMIT`, contenu uniquement selon la vue autorisée, masquage et reçu dans la transaction du Store. Seuls l'ordre et les bornes diffèrent. La sélection SQL reste dupliquée ; tout durcissement devra être reporté dans les deux lectures. La relecture statique ne prouve pas la compilation de la composition.

## Correctifs de composition

1. **Actualiser dans la barre.** Les publications de Supervision et du Journal mettent à jour `aria-disabled` sur le bouton déjà affiché. La commande reste focalisable, ignore une nouvelle activation pendant une lecture et n'entraîne aucune reconstruction de la navigation. Le scénario `focus` exige désormais cette marque dans les deux vues ; les deux exemptions historiques ont été retirées.
2. **Confirmation de reprise.** Le passage à un onglet masqué efface immédiatement la confirmation et annule sa minuterie différée, même si l'onglet redevient visible avant son échéance. La lecture d'identité déjà en vol peut finir ; elle ne réutilise pas la confirmation interrompue pour rouvrir la vue. Le retour propose un contexte équivalent pour un nouveau clic. Les purges de contexte, droits, compte et session conservent leur retrait des données et de l'intention. Le scénario `resume` couvre une absence de 300 ms entre la première réponse et la première relecture différée, avec absence de données avant le nouveau clic.

Les régions d'annonce, la conservation des lignes inchangées et les invalidations après réauthentification sont conservées. Les tokens de thèmes, le mot-symbole OVPT et le favicon de la base ne sont pas remplacés. Les nouveaux panneaux utilisent les tokens existants ; leur rendu réel dans la composition reste à vérifier.

## Sort des huit constats du handoff

| # | Décision | Limite ou preuve attendue |
|---|---|---|
| 1 | Correctif de la marque d'indisponibilité écrit | `focus` à exécuter sur la composition ; aucun PASS repris de Claude |
| 2 | Interruption immédiate de la confirmation écrite | `resume` à exécuter, dont la nouvelle absence de 300 ms |
| 3 | Guides EN/FR précisés : une commande retirée rend le focus à la note de fin **ou à la ligne de résultat** | Pagination de plus de 100 ressources : couverture navigateur encore `NOT_RUN` ; la graine historique en a 15 |
| 4 | Aucune déclaration de conformité ajoutée | Lecteur d'écran, audit WCAG et mesure de contraste `NOT_RUN` |
| 5 | Comportement des vues exactes antérieures conservé et documenté | Leur Actualiser reste `disabled` pendant la lecture ; pas d'extension de périmètre masquée |
| 6 | Latence historique du direct non interprétée | Aucun essai de performance ou de capacité ; investigation contrôle distincte |
| 7 | Comportement partagé conservé ; reçu sans remise décrit fidèlement | Prérefus de la 1 001e page avant le Store ouvert : exige une correction commune à la lecture avant et récente, hors des quatre lignes de raccord de ce lot |
| 8 | Proposition insérée dans `docs/api.md` en deux hunks additifs | Contrat français et anglais, bornes par traversée, reçu sans preuve de remise, erreurs et compatibilité synchronisés avec les guides |

## Validation de cette composition

L'étape initiale autorise uniquement source/données, Git, SHA et analyse statique de confiance. Aucun npm, Cargo, rustc, code du projet, navigateur, runtime, IdP ou modèle n'est exécuté sur l'hôte. Le harnais hors ligne et le gel de source relèvent du coordinateur ; une exécution requiert son feu vert explicite.

| Vérification | Statut de composition |
|---|---|
| `git diff --check`, périmètre et raccord de quatre lignes | Contrôle statique sans anomalie constatée |
| rustfmt sur les six nouveaux fichiers Rust | `PASS` statique, binaire de confiance 1.99.1 macOS ARM64, `--check --edition 2024`, RC 0 ; journal dans la livraison privée |
| Build TypeScript et tests Node | `NOT_RUN` ; QA frontend hors ligne requise |
| Contrats `supervision_stack_api`, `human_recent_events`, suites voisines humaines | `NOT_RUN` ; QA Rust hors ligne requise |
| `focus`, `resume`, `rights`, `revoked`, `isolation`, `regions` | `NOT_RUN` ; sélection minimale utile aux modifications et aux invalidations |
| Expiration réelle de 300 s, test Rust ignoré et scénario `ttl` | `NOT_RUN` ; ne pas les compter dans une exécution par défaut |
| Revue indépendante de composition | Ouverte, obligatoire avant intégration des lectures autorisées/auditées |

Les 174 tests Node et les 16 scénarios navigateur historiques du handoff qualifient uniquement leur source et leur environnement. Ils ne remplacent aucune ligne de ce tableau. Aucun résultat d'absence d'outillage n'est transformé en PASS.

## Documentation et limites produit

Les guides [EN](../../../../setup/supervision/README.md) et [FR](../../../../setup/supervision/README.fr.md) décrivent les mêmes routes, états, refus, bornes, reçus et commandes. Le [contrat IHM](../../../../ui.md) garde la cible complète à 360° distincte de cette consultation. Le texte API est additif ; il ne remplace pas les autres travaux documentaires du coordinateur.

Les lectures couvrent un seul contexte/tenant autorisé et des snapshots indépendants. `contexts_complete=false`, source absente, erreur ou dernier état périmé restent visibles ; aucun total du tenant ne se déduit d'une page. Les brokers multi-hôtes, transports, disponibilité réelle, exécution de workers et capacité 100 ne sont pas qualifiés. La relation agent/sous-agent ou agent/session reste inconnue quand le contrat ne la fournit pas. Aucun bouton ne crée une autorité.

Le worktree demeure consommé par le coordinateur pour revue, gel et validation. Sa fermeture reste ouverte jusqu'à intégration vérifiée et conservation des preuves ; cette étape ne supprime aucun checkout, volume ou travail existant.
