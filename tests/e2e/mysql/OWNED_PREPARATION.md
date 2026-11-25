# G06 — recette d’installation initiale MySQL owned

Critères d’acceptation tirés de `d2e3a277` pour une base dédiée fournie
par l’opérateur. L’installation sélectionne directement le format 14 ou 15.
[Plan G06](../../../docs/engineering/IMPLEMENTATION_PLAN.md#stockage-sql-configurable)

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
compose le [DDL d’audit partagé](../../../schema/mysql/audit-v1.sql), le
[DDL des reçus de décision](../../../schema/mysql/decisions-v1.sql) et le
[DDL MySQL des projections de revue](../../../schema/mysql/review-owned-v1.sql)
avant les [réservations worker](../../../schema/mysql/workers-v1.sql), puis
l’observation et l’allocation du [cycle de vie worker](../../../schema/mysql/worker-lifecycle-v1.sql).
Les tables de [supervision](../../../schema/mysql/supervision-v1.sql) et de
[supervision worker](../../../schema/mysql/worker-supervision-v1.sql)
suivent dans cet ordre. Les cinq [projections egress](../../../schema/mysql/egress-owned-v1.sql)
sont créées en dernier et exclusivement au format 15.
Après les créations internes, la même connexion verrouillée relit dans une
transaction de lecture les douze tables InnoDB du format 14 ou les dix-sept
du format 15, avec leurs colonnes, index et contraintes. Une table egress
en 14 ou une table egress manquante en 15 entraîne un refus. Le contrôle
compare l’expression qui réserve l’unicité aux bindings actifs et les
clés étrangères, y compris les colonnes ordonnées des liens composites.
Le reçu de décision est unique pour une demande et sa phase. Son lien
vers la chaîne d’audit et les deux contraintes de validation sont vérifiés.
Les tables worker, tentative et mutation ont leurs index et références
attendus. La colonne de rattachement de la mutation porte son index
explicite pour obtenir le même catalogue lors d’une installation neuve.
Deux index non uniques couvrent les liens de l’observation vers la tentative
et le worker. Un troisième couvre le lien de l’allocation vers le worker.
L’allocation est unique par tentative et par observation terminale, et son
snapshot conserve le contrat d’allocation non facturable.
Les deux projections de supervision restent vides. Les index primaires
et uniques déclarés couvrent leurs trois clés étrangères.
Toutes les tables doivent être vides, y compris les singletons et projections.
Une reprise refuse ce socle comme cible partielle. Le compte opérateur doit
pouvoir référencer les tables de la cible et garder `foreign_key_checks` et
`unique_checks` actifs dans la session d’installation.

- **Formats 14 et 15.** Le DDL MySQL reprend les colonnes des projections
  [`review_authority_meta`](../../../src/audit/review_authority.rs) et
  [`runner_review_bindings`](../../../src/audit/review_binding.rs). Une
  colonne générée nulle pour les bindings révoqués porte les deux index
  uniques des bindings actifs. Le rejeu MySQL reste à écrire avec une
  ancre de création neuve. Le lecteur SQLite valide une ancre de conversion
  13→14 et le dépôt MySQL ne publie aucune ancre équivalente.
- **Format 15.** Le DDL reprend les clés tenant/session, les références à
  l’audit et la réservation fixe d’un événement et de 98 304 octets. La clé
  réservation → tête est immédiate sur MySQL. Le futur écrivain devra
  comptabiliser la réservation prévue dans la transaction avant l’append,
  puis écrire la révision et la tête avant la ligne de réservation. La genèse,
  le rejeu durable et la validation de cette séquence restent à livrer.
  [Schéma SQLite 15](../../../src/audit/egress/schema.rs) ·
  [Ordre des écritures SQLite](../../../src/audit/egress/mutation.rs)
- **Publication commune.** Les projections MySQL existantes couvrent une partie
  du socle. Le lecteur MySQL d’origine actuel passe par le binding v1 et les
  relectures Legacy. Il reste à attester ensemble tous les marqueurs du
  format, la chaîne, le tenant, le namespace, l’origine physique, la tête
  et l’epoch avant de publier le singleton. Les droits runtime restent
  bornés à leur tenant. [Lecteur MySQL](../../../src/mysql_storage/audit.rs) ·
  [Fixture Legacy](GATEWAY_FIXTURE.md)

## Raccord de l’ancre neuve avant publication

1. Figer la demande depuis la configuration opérateur de confiance. Elle doit
   nommer le format exact, le tenant, le namespace et les UUID de
   l’installation et de la source, ainsi que la cible physique attendue
   (UUID serveur et schéma canonique). Réserver un identifiant d’opération
   stable et lier son contenu à la même demande au rejeu. Définir le
   discriminant et les octets de l’événement de création neuve avant de les
   écrire. `SourceAnchorV1` valide `from_format = 13` et l’ID de migration
   comme ID d’événement. La genèse egress SQLite décrit également une
   transition et porte un ID de migration. Réutiliser les reçus `seq/hash`,
   le tenant, le namespace et les liens de préfixe dans une forme neuve
   explicite. Le reçu d’ancre sert ensuite au descripteur privé attendu.
   Le préparateur interne vérifie déjà la demande d’ancre avant le DDL et
   peut refuser un historique divergent lors d’une réentrée. L’écriture de
   l’ancre et la publication du singleton restent à raccorder.
   [Ancre SQLite](../../../src/audit/review_authority.rs) ·
   [Genèse egress SQLite](../../../src/audit/egress/types.rs) ·
   [Ancre neuve MySQL](../../../src/mysql_storage/owned_origin/fresh_anchor.rs)
2. Fermer le manifeste commun et ses marqueurs. Les douze ou dix-sept tables
   actuelles forment le socle audit, décision, revue, réservation worker,
   cycle de vie et supervision, avec egress en format 15. La genèse et le
   rejeu des marqueurs de supervision restent à raccorder côté owned.
   `control_projections()` exige identité et workload, tandis que le registre
   broker reste facultatif dans ce lecteur. Fixer les capacités owned
   obligatoires et les intégrer au manifeste exact avec les DDL MySQL
   existants et leur rejeu. `verify_empty_in_transaction` relit le manifeste
   actuel et l’absence de lignes dans la transaction appelante. Il vérifie
   avant et après lecture la session, le détenteur du verrou, l’UUID serveur
   et le schéma courant canonique. Il refait la preuve de visibilité du
   catalogue et vérifie les contraintes activées dans cette session. Le garde
   devra couvrir le manifeste étendu avant que l’écrivain puisse l’utiliser.
   [Garde de préparation](../../../src/mysql_storage/owned_origin/preparation/audit_manifest.rs) ·
   [Lecteur MySQL](../../../src/mysql_storage/audit.rs)
3. Construire le vérificateur des préfixes owned et le lecteur de singleton
   exact 14/15. Réutiliser le codec d’événement et de hachage, le calcul de
   `AuditHead`, la lecture de chaîne et les rejeux MySQL compatibles. La
   genèse `control.mysql.audit.genesis` lie déjà tenant et namespace à la
   séquence 1. Si le marqueur de supervision actuel est retenu, il doit
   rester à la séquence 2. Le lecteur `binding` de Legacy doit continuer
   à n’accepter que 1.
   `verify_bound` peut inspirer la relecture d’un binding préparé en mémoire
   avant insertion du singleton, après séparation explicite des marqueurs
   owned. Rejouer les projections d’autorité et de binding à partir des
   événements, puis comparer leurs lignes et leurs empreintes. En 15,
   rejouer aussi la genèse egress. Elle doit référencer l’ancre antérieure
   exacte, le même tenant et namespace et le préfixe immédiatement précédent.
   [Codec commun](../../../src/audit/event_codec.rs) ·
   [Rejeu egress](../../../src/audit/egress/replay.rs)
4. Après le DDL, commencer une transaction InnoDB d’écriture bornée sur la
   connexion verrouillée. Vérifier que la cible reste durable et inscriptible.
   Refaire le garde vide avant le premier insert, puis composer
   la genèse audit, les genèses communes convenues, l’ancre neuve et, en 15,
   la genèse egress. Réutiliser la préparation d’événement, le contrôle de
   capacité et l’insertion d’événement existants. Relire les marqueurs après
   masquage, la chaîne, les projections et la tête calculée dans cette
   transaction. Insérer en dernier l’unique singleton avec format, tenant,
   namespace, séquence, empreinte et octets cumulés. Relire ce singleton
   avant `COMMIT`, puis vérifier à nouveau la cible sous verrou dans un
   nouvel instantané avant tout reçu. Le bootstrap Legacy part d’une
   sentinelle 1 et la met à jour. Le nouveau parcours exige une sentinelle
   absente jusqu’à cette insertion. [Bootstrap MySQL](../../../src/mysql_storage/audit.rs)
5. Sur commit ou réponse incertains, rapprocher l’opération exacte par une
   relecture intégrale sous verrou. Un état partiel ou divergent reste
   fermé, sans nouvelle genèse ni DDL de reprise. La résolution d’une
   préparation ne donne aucune autorité d’admission. Le descripteur et le
   propriétaire retenu actuels portent des identités de fichiers POSIX.
   Avant tout raccord runtime, il faut la liaison MySQL explicite à la
   source attendue et sa validation par le véritable propriétaire sous la
   barrière d’epoch. Le verrou SQL atteste la session de la base.
   [Descripteur actuel](../../../src/human_authority/manifest.rs) ·
   [Epoch et propriétaire](../../../src/human_authority.rs)

## Témoins à exécuter sur cibles jetables

- Le test pur `owned_preparation_requires_the_locked_physical_target`
  vérifie que le manifeste reste lié à l’UUID serveur, au schéma canonique
  et à la session qui détient le verrou.
- Les tests purs `owned_audit_manifest_rejects_column_and_index_drift` et
  `owned_audit_manifest_requires_the_exact_enforced_checks` couvrent le refus
  d’une forme divergente. `owned_review_manifest_rejects_changed_binding_and_audit_links`
  vérifie le refus d’une expression générée ou d’une cible de clé étrangère
  divergente. `owned_review_ddl_preserves_active_uniqueness_and_audit_references`
  vérifie les clauses du DDL de revue. Les tests purs
  `owned_egress_ddl_is_selected_only_for_format_15` et
  `owned_egress_manifest_rejects_wrong_format_and_composite_edges` couvrent
  la sélection du bloc et le refus d’un catalogue ou d’un lien composite
  divergent. L’oracle ignoré
  `owned_audit_tables_remain_unpublished_and_refuse_reentry` exerce le
  sous-manifeste SQL sur des cibles séparées, puis le refus de reprise et le
  refus Legacy. Il reste à exécuter sur MySQL 8.4.
- Sur les deux cibles jetables de cet oracle, relever les `CHECK_CLAUSE`
  du cycle de vie et de la supervision. Pour le protocole, le manifeste
  attend le rendu `length` et l’introducteur `_binary'broker_supervision_v1'`.
  Rapprocher `STATISTICS` du manifeste, dont les trois index non uniques
  explicites du cycle de vie et les clés primaires et uniques de supervision.
  Vérifier l’absence d’index implicite supplémentaire et les sept liens
  du cycle de vie et trois liens de supervision dans `KEY_COLUMN_USAGE`
  et `REFERENTIAL_CONSTRAINTS`. Sur des copies jetables des tables vides,
  ajouter séparément un index surnuméraire au cycle de vie et à la
  supervision, puis appeler `verify_unpublished` sous verrou pour attester
  le refus du manifeste d’index.
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
