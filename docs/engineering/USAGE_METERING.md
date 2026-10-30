# Contrat de mesure d'usage

Le lot de registre courant implémente un ledger d'allocation dans chaque base SQLite v2. Sa qualification intégrée reste décrite dans [CURRENT_STATE.md](CURRENT_STATE.md) ; le précédent candidat déployé ne contient pas ce ledger. Ne pas interpréter les déclarations fournisseurs de tokens ou la durée d'une commande comme une consommation CPU réellement mesurée.

Le contrôle authentifié doit attribuer à chaque tenant et worker les réservations CPU/RAM et leur intervalle d'allocation issu du runtime de confiance. Les unités conservent source, bornes, identités d'idempotence et provenance. Un replay identique ne double pas l'usage ; un contenu divergent pour la même identité est un conflit. La transition terminale, la libération du quota, la mesure et l'audit obligatoire doivent être atomiques et durables.

Un arrêt sans observation terminale n'a pas une durée zéro. Il conserve un état à réconcilier, avec valeur inconnue et capacité réservée selon la politique publiée. CPU alloué × temps et RAM allouée × temps sont des allocations, distinctes des ressources réellement consommées. Les unités portent `billable=false`. Les tarifs, prix et montants financiers restent absents avant validation du compteur, des reprises et des règles commerciales.

Preuves à développer : redémarrage, livraison désordonnée, doublon, conflit, arrêt brutal, dérive d'horloge, audit indisponible, comparaison aux timestamps runtime et réconciliation. La recette locale emploie trois workers ; la capacité cent workers et l'autorité de mesure cloud exigent des essais distincts.

## Format livré par le registre

`GET /api/v1/usage` renvoie une page `{items,next_cursor}`, limitée à 100 lignes. Chaque `AllocationV1` du [contrat Rust](../../src/control.rs) lie `worker_id`, `attempt_id` et `terminal_observation_id`, les bornes runtime `started_at`/`finished_at`, `method_version:1` et `billable:false`. Les montants `duration_ns`, `cpu_millicore_nanoseconds`, `memory_byte_nanoseconds` sont des **chaînes décimales d'entiers exacts**. Ils ne doivent pas être convertis en `Number` JavaScript avant calcul ; le dashboard utilise des entiers et affiche les unités d'allocation.

La source conserve `authority_id`, `runtime_kind`, `container_id`, `spec_hash` et `provenance:observed`. La durée est l'intervalle de processus `StartedAt` → `FinishedAt` inspecté par le broker, sans préparation/import/export. Le calcul multiplie cet intervalle par le CPU et la mémoire alloués inspectés, sans lire un compteur de CPU consommé. Les horloges d'observation du poste et du moteur peuvent différer ; elles ne sont pas substituées aux deux bornes runtime.

Le registre refuse de produire une allocation en cas d'observation incomplète, de restart ou d'écart de ressources. Le premier `StartedAt` et une incohérence déjà observée restent mémorisés : une observation ultérieure apparemment propre n'efface pas l'incertitude. En l'absence de preuve terminale acceptable, l'état et le quota restent à réconcilier, sans ligne à zéro. La transition terminale valide, sa ligne unique par tentative, la libération du quota et l'audit sont atomiques. La réouverture vérifie les projections contre l'audit ; une divergence bloque les opérations.

Les tests de stockage couvrent doublons, conflits, transactions, incohérences et réouverture ; un premier essai Docker du nouveau parcours confirme le refus HTTP de l'observation terminale, l'absence d'allocation et la conservation du quota et du conteneur. Ce résultat ciblé ne qualifie ni toutes les reprises, ni une panne disque, ni une autorité de facturation cloud. Aucun prix ou prélèvement financier n'est activé.
