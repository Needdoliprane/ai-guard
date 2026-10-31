# Revue indépendante — intégration documentaire broker-service

Date : 1 novembre 2026. Checkout : `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, branche `codex/broker-service`, HEAD observé `6c7b63ebcef4a92cadfc2ec95626033f48011576`.

## Périmètre et méthode

Revue documentaire ciblée de quinze fichiers : AGENTS.md, PROMPT_PRODUCTION.md, README.md, docs/sso.md, docs/ui.md, docs/quickstart.md et engineering ARCHITECTURE_CONTRACT, CURRENT_STATE, DECISIONS, IMPLEMENTATION_PLAN, PRODUCTION_READINESS, RELEASE_CHECKLIST, SSO_OIDC, TEST_STRATEGY, VALIDATION.

Comparaison par `git diff --no-index` avec les documents du checkout principal `/Users/louis/Documents/ai-guard` (branche main observée) et la sauvegarde avant fusion `/private/tmp/ai-guard-governance-integration.T9jCW4/branch`. Les suppressions ont été relues séparément des ajouts pour rechercher les pertes d’exigences et d’historique. SSO_OIDC.md n’existe pas dans main et n’est pas dans la sauvegarde branch listée ; son delta a donc été comparé à HEAD et ses mécanismes/propositions relus dans le fichier actuel. Les lignes ci-dessous sont celles du candidat relu.

Aucune modification du dépôt, aucun test applicatif, Cargo, npm, navigateur, moteur, fournisseur, Keycloak, service OS ou accès cloud/PKI lancé par le reviewer. Aucun `.env`, secret ou credential ouvert. Le seul fichier créé est ce rapport privé, hors dépôt, mode 0600. Les lectures ponctuelles de source servent seulement à vérifier les chemins et la réalité des commandes documentées ; elles ne constituent pas une revue applicative.

## Conclusion et constats

**Aucun P0/P1 identifié et aucune correction documentaire bloquante étayée dans ce périmètre.** Aucune perte des exigences demandées, promotion silencieuse de proposition en décision approuvée, commande SSO fictive ou qualification production indue n’a été trouvée. Cette conclusion reste une revue ciblée, sans audit global ni validation des parcours absents.

1. **Autorités et absence de livraison.** AGENTS.md:79–99, PROMPT_PRODUCTION.md:7 et README.md:15 conservent la cible complète, la référence canonique et les statuts « à implémenter et qualifier ». AGENTS, PROMPT, README et ui sont identiques au main courant lors de la comparaison ; les ajouts de branche dans les autres fichiers préservent les obligations récentes. docs/sso.md:7–17 distingue explicitement les clés runner/viewer livrées des identités nominatives, de PKCE et des profils Keycloak/inférence ABSENTS ou NOT_RUN. quickstart.md:9 et :15 exclut toute interprétation des comptes publics comme jeton viewer actuel.
2. **Quatre comptes et permissions.** docs/sso.md:79 et :93–108, SSO_OIDC.md:100–115, TEST_STRATEGY.md:15–17 et IMPLEMENTATION_PLAN.md:389 présentent la même liste : admin/admin pour le realm, admin-ai-guard/admin-ai-guard avec rôle client applicatif, user-1/user-1 et user-2/user-2 sans ce rôle. Activation explicite, loopback, version épinglée et préservation des credentials/bases/volumes sont conservés. Un compte nommé, l’administration du realm ou un rôle valide n’accordent ni runner, ni permission inter-tenant implicite. Les anciennes valeurs user/user et les noms de rôle remplacés sont retrouvés uniquement dans DECISIONS.md:17/:79 et VALIDATION.md:645–647, avec remplacement explicite ; ce sont des références historiques, pas une seconde procédure active.
3. **Mapping et portée Rust.** docs/sso.md:75–83 et SSO_OIDC.md:37–67 conservent les clients publics distincts, S256, absence de client_secret distribué, validation serveur des access tokens, distinction ID token/access token, mapping configurable des chemins de claims et valeurs externes, refus de mapping absent/ambigu, memberships et périmètre organisation/tenants administrés. Le rôle externe de découverte n’est pas imposé aux autres installations. La panne IdP n’active aucune identité ou dépense de repli. Les identités humaine, workload, agent et sous-agent restent distinctes, avec audit obligatoire et sans réattribution des anciennes clés.
4. **Proposition Web toujours non approuvée.** SSO_OIDC.md:29 exige de figer le propriétaire de la transaction et ne décide pas automatiquement d’un BFF ou cookie de session. La proposition de dashboard propriétaire du PKCE, cookie de corrélation et remise unique demeure explicitement une proposition non implémentée à :117–123 ; :121 précise qu’elle ne devient pas une frontière approuvée par sa seule présence. ARCHITECTURE_CONTRACT.md:26 et la dernière phrase de PRODUCTION_READINESS.md réaffirment cette limite. State/nonce/verifier, continuité au redirect, consommation unique, callback CLI loopback, réseau IdP/JWKS borné, guards Origin/callback, refresh/logout/révocation/SSE et migration restent conservés ; aucune mécanique n’est annoncée livrée.
5. **Vues et retrait des données.** docs/ui.md:32–51 et SSO_OIDC.md:59–77 conservent vue personnelle, administration et synthèse à 360° des seuls périmètres autorisés, agrégation Rust, bases séparées, droits distincts métadonnées/contenus/export/mutations, fraîcheur et sources absentes visibles. Retrait de rôle/mapping/membership ou changement de contexte retire les données interdites, détails, caches et flux ; les réponses tardives ne réintroduisent pas l’ancien contexte. Les URL/API/recherche/export/SSE restent contrôlés côté serveur. Le dashboard viewer actuel n’est pas présenté comme la livraison de ces vues.
6. **Axes et inférence interne.** docs/sso.md:27–69/:85–91, TEST_STRATEGY.md:11–13, RELEASE_CHECKLIST.md:8/:29 et SSO_OIDC.md:7 conservent les axes indépendants, exemples non exhaustifs et refus de combinaisons incompatibles/non qualifiées. Dockerfiles et distribution/construction des images par le client n’exigent aucune image publique. Le profil interne lie URL, modèle, format exact Responses/Chat Completions et adaptation Claude qualifiée, référence de credential, chaîne publique d’AC et destinations approuvées ; ni clé privée d’AC, ni désactivation TLS, ni téléchargement/repli cloud implicite. Le profil demeure à implémenter et qualifier.
7. **Preuves historiques préservées.** La comparaison avec la sauvegarde ne révèle pas de suppression des résultats H4–H10, RED/FAIL, limites D5 ou résultats logiciels. Le paragraphe initial de CURRENT_STATE a changé de contexte, mais conserve son checkpoint fa75d6…, la copie initiale et leurs limites à :3 ; VALIDATION.md:506 conserve aussi la preuve datée du push. Les recettes fournisseur vieillissantes du README sont remplacées par le parcours géré du main et leurs résultats datés restent dans VALIDATION. Le gel frontend initial 11/11 demeure dans VALIDATION.md:743 ; son complément final 12/12 est distinct à :749, avec Node25, Node24/DOM/navigateur/raccord runtime NOT_RUN. Les compteurs de tests ne sont pas additionnés entre ces gels. G09 reste FAIL, les autres gates conservent leurs écarts et 0/12 gates sont fermés.
8. **Traçabilité de la fusion.** CURRENT_STATE.md:9 décrit l’intégration de gouvernance sans livraison SSO. La fin de VALIDATION distingue la revue antérieure du main de l’intégration à trois versions dans broker-service ; la phrase historique « Aucun code ou résultat du worktree importé » est ainsi clairement bornée à la revue main. La dernière phrase de PRODUCTION_READINESS conserve explicitement discovery/JWKS/transport IdP G05/G11, sessions/fin SSE G08 et migrations/historique G06, ABSENTS/NOT_RUN. Aucun ancien PASS n’est transféré au candidat par cette rédaction.

## Commandes et liens ciblés

PASS documentaire constaté : `git diff --check -- <les quinze fichiers ci-dessus>` termine sans diagnostic. Les comparaisons utilisent diff et recherche textuelle, sans exécution des commandes produit.

Les nouveaux renvois importants ont été confrontés aux fichiers et titres présents : état livré de sso.md, axes indépendants, inférence interne, profil Keycloak, fiche opérateur SSO_OIDC, vues utilisateur/administration, lot identité du plan, matrice identité des tests, cycle broker permanent du plan, H10 et complément final des étapes natives. Les ancres du guide fournisseur et de l’état livré PROVIDER_AUTH ont aussi leurs titres. Aucun lien ciblé cassé identifié. Ce contrôle est manuel et ciblé, pas un validateur exhaustif de tous les liens/ancres.

La commande `broker recovery-status` et ses options root/profile/token-file/operation-id, ainsi que `broker cancel --command-id` et la ligne `Command: <UUID>`, sont présentes dans `src/broker_cli.rs:21–84/:112–115/:214–249` ; elles ne sont pas des options SSO inventées. Elles n’ont pas été exécutées. Les valeurs de fiche opérateur OIDC restent un inventaire à renseigner, explicitement pas un schéma actuellement accepté.

## Gel et changements pendant la revue

Le coordinateur a complété pendant la lecture les paragraphes frontend12 et l’encadrement de la fusion dans CURRENT_STATE, IMPLEMENTATION_PLAN, TEST_STRATEGY, PRODUCTION_READINESS et VALIDATION ; leurs derniers paragraphes pertinents ont été relus. docs/sso.md a changé uniquement par correction grammaticale à :75 (« validées » → « validés »), signalée au coordinateur. Le hash initial 9577f40… est exactement retrouvé en rétablissant ce seul accord en mémoire ; aucun changement de fond SSO n’en est déduit. SSO_OIDC et ui sont restés stables.

SHA256 finaux du périmètre relu, avant l’ajout ultérieur du résultat de cette revue :

| Fichier | SHA256 |
|---|---|
| AGENTS.md | ae57ce8c0a244ea5196487435f0d90677b6b4d736c9d99d2f4893035a0f81216 |
| PROMPT_PRODUCTION.md | 4e84efae48ae70d01a404316e15d9f880c662307f8b89da34a9c8031efaff377 |
| README.md | d35d5c19158985de780ea188532f9389e22dc521f07a765c900714b38c089106 |
| docs/sso.md | 366c5064580653f2263137311b4fa74a6cc7fd1fd1b9d5f98575a231e57e60b4 |
| docs/ui.md | 7e97f8e4d9c7bee7e2729a279b49542fa8a8bd0c94c233acb6205f82f5a21b40 |
| docs/quickstart.md | 6e4bb27bcc2a3470794cffd8e7e340b0c537ea475068a814620f1ca672abda42 |
| docs/engineering/ARCHITECTURE_CONTRACT.md | 82033f240543e4e38becc5df7885aa8a7684efd6fd9bb7dea91f1fd458dc922c |
| docs/engineering/CURRENT_STATE.md | a5e2b360dbe3ad57d30018543bec2485be0859435c5a07a5a08aa9872ae97a1e |
| docs/engineering/DECISIONS.md | 1476c35247dbe819aff88175c5bf292fec634440c552782606fc1b9d2521dd5d |
| docs/engineering/IMPLEMENTATION_PLAN.md | 6c45d3e31d310ea967786beb0e78994ebc2d0045c2597ce9432a73ba58468537 |
| docs/engineering/PRODUCTION_READINESS.md | 4a9836c153ed7dc97e8d1c7a58a0df9728f9b8c142998b29de3b38f4f1c9d40a |
| docs/engineering/RELEASE_CHECKLIST.md | 3f6df96ef59045789757863ee0b2ea8e561a77e4775dfab3f252c6d6d7b7691f |
| docs/engineering/SSO_OIDC.md | f324943d1447678cde3f1ae971965a6800be3b1ba8e6bd0cfed010bbcd14865b |
| docs/engineering/TEST_STRATEGY.md | 0f93eaa99f070ea00150fe112a50f9a73050095f8cab0f18f21a9eb6a9dcac05 |
| docs/engineering/VALIDATION.md | 8726593cf86061e5c2e483b85a2a775634d69661aa505cf8bacfbdad2f21844e |

## Limites

NOT_RUN : tous les parcours applicatifs, les tests, Node24/navigateur, Keycloak/OIDC, inférence interne et les effets runtime. Les nombres de résultats datés sont contrôlés pour leur cohérence documentaire, sans rejouer ni requalifier leur preuve dans cette mission. Les réponses HTTP des références externes, toutes les ancres historiques et l’intégralité du code hors périmètre ne sont pas vérifiées. Le résultat de contrôle global des liens annoncé séparément par le coordinateur n’est pas revendiqué comme une exécution du reviewer. Aucun gate ni release qualifié par cette revue. Blocage documentaire propre à ce périmètre : aucun identifié.
