# Préparer une session et envoyer une consigne

[English](README.md)

## Dashboard avec sources owned

Utilisez un dashboard et un gateway configurés en `owned_sources`. Connectez-vous avec un compte nominatif et sélectionnez un contexte personnel. Demandez à l’opérateur l’identifiant opaque de la cible de lancement et sa révision courante. Le contexte doit autoriser chaque action séparément. [Sources owned](../identity/OWNED_SOURCES.fr.md)

1. Ouvrez **Nouvelle session IA**, saisissez la cible et la révision publiées, puis choisissez **Préparer le lancement**. Vérifiez la préparation retenue, son expiration et son reçu d’audit. La préparation laisse le worker et le quota intacts.
2. Choisissez **Lancer la session** pour valider cette préparation. Le reçu d’acceptation identifie la session et son grant nominatif. Consultez les observations du broker et du runtime pour établir l’admission et le démarrage.
3. Si le résultat est inconnu, utilisez **Réessayer le même lancement** depuis le même contexte. Cette action reprend l’opération et la préparation déjà soumises. Le contrôle peut restituer un reçu existant après expiration de la préparation. Un refus pendant la réconciliation laisse l’issue initiale inconnue.
4. **Envoyer une consigne** exige une liaison de conversation courante pour la session, le worker, la tentative et l’époque d’entrée exacts. Vérifiez la destination avant l’envoi. Relisez le reçu pour distinguer enregistrement, transmission, acquittement et livraison inconnue. L’acquittement porte sur le protocole du fournisseur. Observez séparément les actions qui en découlent.

Pour retrouver une session existante, ouvrez **Ressources** et choisissez **Consulter le canal** sur une session dont vous êtes propriétaire. Le dashboard relit la liaison auprès du gateway. Choisissez **Utiliser ce canal** quand il est actif et que son échéance permet encore l’envoi. **Relire le canal** vérifie son état courant. Si l’époque d’entrée change, sélectionnez explicitement le nouveau canal.

Si votre contexte autorise l’envoi et ne propose pas la vue Ressources, ouvrez **Sessions** et saisissez l’identifiant exact d’une session dont vous êtes propriétaire. Le gateway vérifie le contexte, le tenant et la propriété de la session.

L’implémentation actuelle couvre les opérations durables du plan de contrôle et les vues du dashboard. Les tests du composeur du broker utilisent un enfant stdio contrôlé et un vrai Store owned. Le raccord à l’admission supervisée de production et les parcours complets avec runtimes et fournisseurs restent ouverts. [Contrat de session](../../engineering/SESSION_CONTROL.fr.md) · [Gates de qualification](../../engineering/PRODUCTION_READINESS.md)

## Préparer et confirmer sur SQLite Legacy

Le CLI Legacy utilise un compte local nominatif et un contexte personnel doté de `launch_session`. L’opérateur provisionne une cible fixe pour ce compte et son organisation. Cette procédure couvre la préparation et la confirmation. L’admission par le broker et le démarrage par le runtime restent ouverts pour ce profil.

### Maintenance opérateur

1. Utilisez la base SQLite existante du tenant, de format 10 à 13. Arrêtez le gateway, les brokers et les redémarrages automatiques avant d’indiquer `--confirm-control-stopped`. Si nécessaire, installez d’abord le marqueur `human_rights_v2` avec `migrate-human-rights` et une nouvelle sauvegarde distincte. [Migration des droits humains](../identity/README.fr.md#activer-les-capacités-des-droits-humains)
2. Renseignez `tenant_db`, `tenant_id` et `audit_limit` avec le chemin de la base, le tenant et la limite d’événements configurée. Choisissez un chemin `prepare_backup` libre dans un répertoire privé, puis installez la capacité de préparation distincte.

   ```sh
   agentsandbox enable-legacy-session-prepare \
     --data "$tenant_db" --tenant-id "$tenant_id" --max-events "$audit_limit" \
     --backup "$prepare_backup" --confirm-control-stopped
   ```

   Conservez la sauvegarde vérifiée et le rapport JSON. `from_version` et `to_version` indiquent tous deux le format de la base existante. `backup_path`, `genesis_seq` et `genesis_hash` identifient la sauvegarde et le marqueur. Une seconde installation est refusée. En cas de perte du rapport, conservez la sauvegarde et recherchez `control.legacy_session_prepare.genesis` dans l’audit du tenant avant toute nouvelle tentative. La migration actuelle vers `owned_sources` refuse une base qui porte ce marqueur. [Maintenance des sources humaines](../human-sources-maintenance/README.fr.md)

3. Avec le contrôle toujours arrêté, provisionnez la cible depuis un fichier JSON privé désigné par `target_file`. Pour une nouvelle cible, utilisez `expected_revision: 0` et la révision de catalogue `1`. La cible fixe les comptes locaux autorisés, l’incarnation du broker et la révision de son grant, ainsi qu’une spécification de lancement `generic` avec `fixed_sandbox_v2` et `network: none`, sans credential fournisseur. [Format de la cible](../../../src/control/session_control.rs) · [Grants broker](../workloads/README.fr.md#préparer-un-grant-ou-une-révocation-de-broker-par-cli)

   ```sh
   agentsandbox human provision-legacy-session-target \
     --data "$tenant_db" --tenant-id "$tenant_id" --max-events "$audit_limit" \
     --target-file "$target_file" --confirm-control-stopped
   ```

   Le reçu JSON contient `target_id`, `target_revision`, `audit_seq` et `audit_hash`. Réutiliser le même fichier de cible et la même révision restitue le reçu d’audit d’origine avec `replayed: true`. Transmettez l’identifiant et la révision de la cible à l’utilisateur nominatif. Remettez le gateway Legacy en service pour les demandes humaines ci-dessous.

### Demande nominative et rejeu

Indiquez l’origine du gateway Legacy dans `gateway` et votre fichier privé de credential local dans `credential_file`. Conservez le credential et les demandes dans des fichiers réguliers de mode `0600` appartenant au compte qui exécute le CLI, dans un répertoire privé. HTTP est disponible sur loopback. Pour HTTPS avec une AC privée, passez `--gateway-ca-file` avec la chaîne publique d’AC approuvée à chaque commande.

```sh
agentsandbox human identity --gateway "$gateway" --credential-file "$credential_file"
```

Reprenez l’identifiant du contexte personnel dans `context`. Enregistrez `prepare_request` dans un fichier JSON privé avec `version` à `1`, un nouvel UUID `operation_id`, les valeurs `launch_target_id` et `expected_target_revision` fournies par l’opérateur, et une échéance RFC 3339 `expires_at` dans la durée de validité du contexte et du credential.

```sh
agentsandbox human prepare-legacy-session --gateway "$gateway" \
  --credential-file "$credential_file" --context "$context" --request-file "$prepare_request"
```

Vérifiez la cible, la révision, l’échéance et le reçu d’audit. La préparation laisse le worker et le quota intacts et ne crée aucun grant de session. Enregistrez `confirmation_request` dans un fichier JSON privé avec `version: 1`, un nouvel `operation_id` et les mêmes `launch_target_id` et `expected_target_revision`. Recopiez l’`operation_id` de la demande de préparation dans `preparation_operation_id` et l’`input_hash` du reçu dans `preparation_input_hash`.

```sh
agentsandbox human confirm-legacy-session --gateway "$gateway" \
  --credential-file "$credential_file" --context "$context" --request-file "$confirmation_request"
```

Le reçu de confirmation contient `binding_ref`, `session_id`, `audit_seq` et `audit_hash`. Il atteste uniquement d’une confirmation retenue. L’admission par le broker, le grant, la réservation du worker et le claim de démarrage restent à établir.

En cas de réponse perdue, conservez le fichier de demande et rejouez la même commande avec l’identifiant d’opération et le contenu d’origine. Un rejeu réussi indique `replayed: true` et restitue le reçu d’audit d’origine. Le gateway revérifie l’identité locale, l’appartenance et les droits courants sur la cible. Utilisez `human identity` pour obtenir un contexte personnel frais si nécessaire. Rejouer Prepare exige une échéance de demande encore valide. Le gateway peut restituer une confirmation déjà validée après l’expiration de la préparation. Une demande divergente ou un refus d’autorisation laissent l’issue initiale indéterminée.

Pour `migration_required`, vérifiez l’installation du marqueur de préparation. Pour `context_revoked` ou `permission_denied`, vérifiez l’appartenance, le contexte personnel et l’accès courant à la cible avant de réessayer. Pour `conflict`, relisez la révision de la cible et la demande retenue avant de choisir une autre opération.

Les tests sur `6b8d029f` spécifient la sauvegarde, la réouverture SQLite et le rejeu du reçu exact via le routeur Rust. L’exécution de ces tests sur cette révision et la reprise avec un gateway déployé et un vrai broker restent `NOT_RUN`. [Tests](../../../tests/human_rights_migration.rs) · [Contrat de session](../../engineering/SESSION_CONTROL.fr.md) · [Gates de qualification](../../engineering/PRODUCTION_READINESS.md)
