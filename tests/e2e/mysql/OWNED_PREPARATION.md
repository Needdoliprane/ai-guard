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

Provisionner un compte TLS dédié, distinct du compte de service du gateway.
Pour rester au périmètre de la cible, utiliser une seule attribution directe
au schéma exact qui réunit `SELECT`, `SHOW VIEW`, `ALTER ROUTINE`, `EVENT`,
`TRIGGER` et `CREATE`, sans rôle actif. Échapper les caractères `_` et `%`
du nom de la base lorsque `partial_revokes` est désactivé. Lorsqu’il est
activé, le nom de la portée doit correspondre littéralement à la base.
L’opérateur réserve un nom d’utilisateur unique sur le serveur, sans autre
entrée hôte ni droit anonyme concurrent applicable, et maintient les droits
et `partial_revokes` stables pendant le contrôle. `SHOW GRANTS FOR CURRENT_USER()`
détaille les attributions directes du compte courant.

Le prévol accepte aussi les droits globaux directs complets en l’absence de
révocation et sans rôle actif si `partial_revokes` est activé. Pour la preuve
par schéma, une seconde portée schéma rend la sélection incertaine et produit
`CatalogVisibilityUnconfirmed`
avant l’inventaire. Le gateway garde les droits de service sur les objets
préparés. [Sélection des droits MySQL](https://dev.mysql.com/doc/refman/8.4/en/request-access.html) · [GRANT MySQL](https://dev.mysql.com/doc/refman/8.4/en/grant.html) · [SHOW GRANTS MySQL](https://dev.mysql.com/doc/refman/8.4/en/show-grants.html)

## Décision requise avant raccordement

Fixer le manifeste physique de chaque format MySQL, avec tables, colonnes,
index et contraintes, genèses et marqueurs de capacité requis, ainsi que
leur vérificateur owned. Le [DDL Legacy](../../../schema/mysql/audit-v1.sql)
pose une sentinelle v1 et `initialize` en dépend. Le vérificateur d’origine
MySQL actuel utilise encore ce lecteur v1. Valider aussi les droits minimaux
du compte de préparation et les droits de lecture/écriture du compte de
service sur les objets effectivement retenus. Le compte de service ne doit
pas pouvoir créer ni modifier le schéma. La [fixture Legacy](GATEWAY_FIXTURE.md)
ne qualifie pas ce provisionnement owned.

## Témoins à exécuter sur cibles jetables

- Le prévol distingue base vide, préparation interrompue et Legacy. À
  l’installation, comparer inventaire et état durable avant/après chaque refus
  et vérifier qu’aucune instruction mutante n’a été émise sur la cible refusée.
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
