# Capacités de contrôle du broker pour les cibles SQL

**Approuvée par le mainteneur le 21 novembre 2026 ; implémentation en cours dans le lot G06-mysql, aucune livraison ni qualification déclarée.** Le raccordement MySQL ouvre un problème de compatibilité. Le gateway expose `PRAGMA user_version` dans `ControlProtocolV1` et plusieurs consommateurs du broker en déduisent les opérations autorisées. La version physique du schéma MySQL ne porte pas ces garanties. [Contrat actuel](../../src/control/supervision.rs) · [Validation côté broker](../../src/broker/service/mod.rs)

## Contrat proposé

Conserver `ControlProtocolV1` pour SQLite et ajouter une réponse distincte de capacités sur le transport de contrôle privé. La route candidate est `GET /control/api/v1/control/capabilities`. Elle utilise le même filtrage de routes et la même authentification runner. La réponse typée contient sa version de contrat, le rôle, le tenant authentifié, le namespace vérifié et les opérations effectivement prises en charge. [Transport de contrôle](../../src/control_transport.rs) · [Routes de supervision](../../src/api/supervision.rs)

La réponse distingue la consultation de l’état de reprise, l’arrêt ou la réconciliation d’un travail et l’admission supervisée. Le serveur annonce l’admission quand la réservation, l’armement, le claim, les opérations auxiliaires et l’export sont disponibles avec une autorité active. Une cible réouverte dont le témoin manque refuse l’admission. Les contrôles transactionnels d’autorité et de tenant restent appliqués à l’opération elle-même. [État de supervision SQLite](../../src/audit/supervision.rs) · [Origine MySQL](../../src/mysql_storage/audit.rs)

Pour un profil configuré sur le nouveau contrat, le client borne la réponse à 16 Kio. Il vérifie l’origine TLS, le rôle, le tenant et le namespace attendu par le profil avant d’ouvrir le journal. Un refus, une route absente, une réponse invalide ou un écart d’identité termine la demande. Le profil ne déclenche aucun repli vers V1 ni vers l’ingress public. Le gardien décide toujours à partir de ses preuves IPC, d’armement et d’horloge. [Client de contrôle](../../src/runner.rs) · [Tests du transport](../../src/runner/control_transport_tests.rs)

## Première tranche à qualifier

| Parcours | Résultat attendu |
|---|---|
| Broker V1 et SQLite | Même réponse, mêmes rôles et mêmes refus. Les scénarios SQLite actifs et inactifs gardent leurs résultats. |
| Nouveau client et SQLite | Le runner reçoit les capacités compatibles avec l’état vérifié. Le viewer est refusé. Une réponse trop grande, inconnue, absente ou attribuée à un autre tenant ou namespace bloque sans nouvelle requête. |
| Reprise avec nouveau contrat | La lecture de diagnostic, l’arrêt et la réconciliation requièrent les preuves propres à chacune. Un témoin perdu interdit toute nouvelle admission. |
| MySQL TLS et mTLS | La vérification de la chaîne et des projections précède toute capacité. Tester chaque opération annoncée et son rejeu sur une cible préparée distincte avant d’ouvrir le gateway. |

Après validation du contrat, le premier lot relie le DTO, la route privée et la lecture bornée du client. Il exerce le contrat sur SQLite avec le vrai gateway. La qualification de la reprise MySQL et de ses routes métier conditionne son ouverture. Le parcours Kubernetes held-create garde sa propre qualification.

Les sélecteurs `served_control_protocol_versions_match_schema_enum_and_active_constraint`, `trusted_client_refusal_never_retries_on_the_public_transport` et `inactive_real_gateway_allows_fresh_stop_and_recovery_metadata_only` donnent les résultats de référence à conserver. [Compatibilité wire](../../tests/control_protocol_schema.rs) · [Refus de repli](../../src/runner/control_transport_tests.rs) · [Reprise inactive](../../src/broker/service/stop_auth_tests.rs)
