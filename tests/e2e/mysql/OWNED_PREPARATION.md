# G06 — recette d’installation initiale MySQL owned

Critères d’acceptation tirés de `d2e3a277` pour une base dédiée fournie
par l’opérateur. L’installation sélectionne directement le format 14 ou 15.

## Garde et publication

1. Ouvrir une connexion TLS dédiée sur la base configurée. Vérifier l’UUID
   serveur et le nom canonique de la base, prendre le verrou de cette cible,
   puis inventorier tables, vues, routines, événements et triggers avec une
   visibilité démontrée. Garder cette connexion pour le premier DDL, la
   transaction de publication et la vérification finale. Un reçu de prévol
   obtenu sur une connexion fermée ne donne aucun droit d’écrire.
2. Sur une cible non vide, vérifier intégralement le format demandé, son
   origine et l’état durable. Une cible exacte et vérifiée répond en lecture
   seule. Une cible Legacy, divergente ou partielle est refusée avant toute
   écriture. Un verrou perdu ou un inventaire incertain ferme la préparation.
3. Sur une cible vide, exécuter les créations strictes prévues par le format.
   Factoriser les déclarations réellement communes avec le DDL Legacy. La
   sentinelle d’audit reste absente pendant le DDL. L’ancien lecteur doit
   refuser à chaque étape visible. Un arrêt après un DDL peut laisser des
   objets. La prochaine tentative les reconnaît comme un partiel et exige
   l’intervention de l’opérateur.
4. Vérifier le catalogue exact et la capacité à tenir les écritures durables.
   Dans une seule transaction InnoDB, poser les genèses et projections
   obligatoires, puis vérifier leur cohérence avec l’origine, la tête calculée
   et la barrière d’epoch. Publier la sentinelle 14 ou 15 avec cette même
   transaction. Avant le commit elle reste invisible aux autres connexions.
5. Après commit, relire sur la connexion verrouillée, dans un nouvel instantané.
   Confirmer le manifeste du format, le lien serveur/base/tenant/namespace,
   la genèse, les marqueurs obligatoires, la chaîne et les projections
   vérifiées, la tête durable et l’epoch attendues. Le reçu de succès exige
   cette relecture et une libération certaine du verrou. Après une réponse
   perdue, refaire la vérification intégrale sous verrou en lecture seule.
   Une préparation incomplète ou incertaine reste fermée.

Le verrou arbitre les clients ai-guard qui l’utilisent. La recette exige
une cible réservée, sans administration concurrente du schéma ni autre
écrivain pendant la préparation.

## Compte opérateur du prévol

Réserver un compte TLS à l’opérateur chargé du prévol et de la future
préparation. Pour établir la visibilité du catalogue, le prévol exige les
droits directs `SELECT`, `SHOW VIEW`, `ALTER ROUTINE`, `EVENT`, `TRIGGER` et
`CREATE` à la portée globale. Il refuse toute révocation affichée et exige
l’absence de rôle actif si `partial_revokes` est activé. Les droits restent
stables pendant le contrôle. Le gateway et les autres comptes runtime
gardent leurs droits bornés au tenant.

`SHOW GRANTS FOR CURRENT_USER()` décrit le compte courant. La sélection
effective des droits de schéma peut dépendre d’une autre entrée hôte pour le
même nom d’utilisateur. Un grant de schéma exact ou une attestation
d’unicité laisse donc la visibilité non prouvée. Un compte limité au schéma
reçoit `CatalogVisibilityUnconfirmed` avant l’inventaire. [Sélection des droits MySQL](https://dev.mysql.com/doc/refman/8.4/en/request-access.html) · [SHOW GRANTS MySQL](https://dev.mysql.com/doc/refman/8.4/en/show-grants.html)

## Sous-manifeste d’audit et publication restante

Le [sous-manifeste d’audit](../../../src/mysql_storage/owned_origin/preparation/audit_manifest.rs)
couvre les deux tables issues du [DDL partagé](../../../schema/mysql/audit-v1.sql).
Après les créations internes, la même connexion verrouillée relit dans une
transaction de lecture les tables InnoDB, les 13 colonnes, les trois index
et toutes les contraintes, y compris l’expression entière de chaque `CHECK`
et la version exacte choisie. Elle exige l’absence de lignes d’audit, de
singleton et de tout autre objet utilisateur du schéma. Cet état est un socle non
publié. Une nouvelle préparation le refuse comme cible partielle.

- **Formats 14 et 15.** Le DDL MySQL et le rejeu correspondant aux projections
  [`review_authority_meta`](../../../src/audit/review_authority.rs) et
  [`runner_review_bindings`](../../../src/audit/review_binding.rs) restent à
  définir, y compris l’équivalent des deux index uniques filtrés du binding
  actif. Leurs définitions et leur vérification existent côté SQLite.
- **Format 15.** Il manque aussi le DDL MySQL et le rejeu des cinq projections
  `egress_policy_*`, avec leur genèse et leurs réservations durables.
  [Schéma SQLite 15](../../../src/audit/egress/schema.rs)
- **Publication commune.** Les projections MySQL existantes couvrent une partie
  du socle. Le lecteur MySQL d’origine actuel passe par le binding v1 et les
  relectures Legacy. Il reste à attester ensemble tous les marqueurs du
  format, la chaîne, le tenant, le namespace, l’origine physique, la tête
  et l’epoch avant de publier le singleton. Les droits runtime restent
  bornés à leur tenant. [Lecteur MySQL](../../../src/mysql_storage/audit.rs) ·
  [Fixture Legacy](GATEWAY_FIXTURE.md)

## Témoins à exécuter sur cibles jetables

- Les tests purs `owned_audit_manifest_rejects_column_and_index_drift` et
  `owned_audit_manifest_requires_the_exact_enforced_checks` couvrent le refus
  d’une forme divergente. L’oracle ignoré
  `owned_audit_tables_remain_unpublished_and_refuse_reentry` exerce le
  sous-manifeste SQL sur des cibles séparées, puis le refus de reprise et le
  refus Legacy. Il reste à exécuter sur MySQL 8.4.
- Le prévol distingue base vide, préparation interrompue et Legacy. Pour
  `owned_preparation::owned_preflight_refuses_interrupted_and_legacy_schemas`,
  renseigner le compte opérateur dans `maintenance_username` de la configuration
  `AI_GUARD_MYSQL_OWNED_PREPARATION_QA_CONFIG`. La fixture pose la sentinelle
  Legacy et requiert aussi `INSERT` sur le schéma de recette.
  À l’installation, comparer inventaire et état durable avant/après chaque
  refus et vérifier qu’aucune instruction mutante n’a été émise sur la cible
  refusée.
- L’oracle `owned_preparation::legacy_store_refuses_uncommitted_and_owned_version_state`
  exerce le Store Legacy sur deux cibles distinctes. Il couvre base vide,
  premier DDL seul, sentinelle absente, sentinelle encore non commise puis
  version commise. Il simule la publication d’une version avec les colonnes
  historiques. La preuve d’installation exige le vrai manifeste de chaque
  format et le refus de l’ancien binaire à toutes les phases publiées.
- Pour chaque format, prouver l’installation directe, la relecture complète
  et la reprise en lecture seule après perte de la réponse. Vérifier aussi le
  refus sans effet d’un format, d’une identité ou d’une origine divergents,
  et l’absence de seconde publication sous concurrence. Qualifier les deux
  profils de connexion, mot de passe avec TLS et mot de passe avec mTLS.
