# P1 — Une ingestion ne peut émettre une décision du contrôle

29 octobre 2026. Correction limitée à src/api.rs et nouveau tests/decision_event_authority.rs. Aucun changement audit.rs, modèle, moteur ou déploiement. Diff /tmp/ai-guard-decision-event-authority.diff ; baseline API avant changement /tmp/ai-guard-api-before-authority.rs.

## Défaut et portée démontrés

Un appelant disposant du credential **runner de son tenant** pouvait poster un Event security.decision avec source=gateway, provenance=observed, outcome=allow et les métadonnées request_id/request_hash/profile/human_approved/jev attendues par le cache. POST /events vérifiait la forme du document, puis le stockait sans réserver ce namespace. Store::lookup_decision prenait ensuite ce faux événement pour une décision existante.

Reproduction HTTP réelle en loopback, base temporaire et token factice : ingestion du faux allow→200 ; POST /api/v1/decide strict pour argv=[rm,-rf,/workspace] avec le même request_id/hash→allow au lieu du deny déterministe. Le test s'arrête au résultat faux : **aucune commande métier ni conteneur lancé**.

Classification retenue avec root : P1 d'intégrité de l'autorité centrale. Credential runner requis ; ce n'est pas une preuve de contournement worker/viewer, de fuite inter-tenant ou d'exécution hors sandbox. Le contrôle de confiance peut appeler l'API d'ingestion, mais cette route ne doit pas transformer une trace fournie en décision de politique.

## Correction et compatibilité

Les deux alias POST /api/events et /api/v1/events refusent403 les racines suivantes et leurs descendants séparés par un point :

- security.decision et security.decision.* ;
- control et control.* ;
- worker et worker.* ;
- usage.allocation et usage.allocation.*.

La comparaison suit les noms canoniques sensibles à la casse du contrat Event. Un kind ressemblant sans frontière de namespace, tel security.decision_reported, reste une trace sans autorité ; le lookup décision ne considère que le kind exact security.decision. Le refus ne dépend pas de source/provenance fournis : un événement declared/provider ne peut pas prendre ce namespace non plus. Authentification/rôle et validation existantes restent appliquées. La vraie route /decide continue à émettre ses événements en interne via Store::decision.

usage.reported est volontairement conservé : les adaptateurs Codex/Claude/generic l'utilisent pour des unités déclarées, sans allocation facturable ni droit de contrôle. Aucun blocage global de usage.* ou security.*.

Le test de compatibilité ingère14 kinds réels ou voisins sans autorité : command.requested, command.started, command.completed, runtime.submitted, runtime.execution.error, runtime.cleanup.failed, runtime.security_downgrade, workspace.import.completed, workspace.export.completed, agent.declared, tool.requested, tool.result, usage.reported, security.decision_reported. Les événements runner existants sous network.*, observer.*, workspace.*, runtime.*, command.* restent hors des namespaces réservés. Cela ne constitue pas un test moteur de ces parcours.

## Preuves observées

Premier essai temporairement BLOCKED par édition concurrente runner/runtime (execute_retained/result/cleanup manquants). Ce défaut de compilation ne compte pas comme reproduction. Le refus ingestion préparé a été retiré avant l'essai discriminant, puis réappliqué seulement après RED métier.

cargo test --locked --test decision_event_authority avant correction : **RED, deux FAILED et un PASS**. Le test d'exploitation observe allow au lieu deny ; le test namespaces observe200 au lieu403 ; compat14 kinds PASS.

Après correction et formatage :

- cargo test --locked --test decision_event_authority --test decision_idempotence --test api_security --test tenant_api --test jev_contract : **PASS21**, zéro échec/ignoré (3+9+4+3+2).
- cargo clippy --locked --lib --test decision_event_authority -- -D warnings : **PASS ciblé**.
- rustfmt --edition 2024 --config skip_children=true src/api.rs tests/decision_event_authority.rs : **PASS**.

Le scénario corrigé refuse l'ingestion403, puis la vraie décision retourne deny et la base contient seulement cette décision légitime. Les deux alias et une provenance déclarée sont couverts. Suite globale et revue indépendante à intégrer par root ; aucune qualification production ou moteur annoncée.

## Historique et limite incontournable

Cette correction empêche les nouvelles ingestions usurpées. Elle **n'assainit pas les bases existantes** et ne requalifie pas les anciennes décisions comme authentifiées : un ancien événement forgé peut être identique à un événement réellement émis par /decide. Aucun marqueur de route ou reçu d'autorité distinct n'existait ; une recherche par kind/source/provenance ne peut trancher cette origine. Les événements, hashes et séquences sont conservés, sans suppression, réparation ni réécriture de l'historique.

Pour v2, un genesis de contrôle et des méthodes dédiées doivent séparer la projection du registre des traces libres. Une admission versionnée doit réévaluer/obtenir une **nouvelle autorisation centrale complète** du LaunchSpec ; elle ne convertit jamais un vieux allow ou un simple événement security.decision en droit start. Les anciennes décisions peuvent rester lisibles/idempotentes au contrat existant, avec cette limite explicitement visible.
