# Revue indépendante — autorité des événements de décision

29 octobre 2026. **PASS au périmètre de prévention des nouvelles ingestions réservées. Aucun contournement confirmé dans le diff relu.** La limite historique reste ouverte ; ce verdict ne constitue pas une qualification globale de l'autorité de lancement ou du produit.

Périmètre : `src/api.rs` et `tests/decision_event_authority.rs`, diff `/tmp/ai-guard-decision-event-authority.diff`, baseline `/tmp/ai-guard-api-before-authority.rs`. Lecture des consommateurs directs dans audit/modèle/adaptateurs/runner pour suivre la frontière. Aucun fichier produit, moteur, volume ou endpoint partagé modifié. Skill `security-audit` utilisé comme guidance de revue ciblée ; couverture limitée à ce diff et ses chemins directs.

## Frontière et résultat attendu

Le principal concerné dispose déjà du credential runner de son tenant. L'ingestion de traces libres ne doit pas lui permettre de créer un événement que le cache interprète comme une décision centrale. La ressource affectée est la décision du même tenant. Le RED auteur démontre un faux allow ; aucune exécution métier, intrusion worker/viewer ou fuite inter-tenant n'en est déduite.

Le diff ajoute uniquement un refus avant `Store::append`, après authentification et validation du même objet Event. Les opérations centrales continuent à écrire en interne via `Store::decision`.

## Critères et recherche de contournement

- **PASS — Alias.** `gateway_router`, lignes 272–294, construit un seul jeu de routes puis le monte sous `/api` et `/api/v1`. Les deux POST events utilisent le même handler et la même garde ; aucune seconde route d'ingestion libre trouvée dans le périmètre.
- **PASS — Frontière des namespaces.** Racines exactes `security.decision`, `control`, `worker`, `usage.allocation`, ou suffixe commençant par un point, refusées 403 avant append. Les descendants à plusieurs points sont inclus. Un voisin comme `security.decision_reported` n'est pas une décision centrale et conserve sa compatibilité.
- **PASS par trace de source — Casse/encodage/structure.** La garde porte sur le kind déjà désérialisé, ensuite persisté depuis ce même objet ; une représentation JSON échappée du point ne crée pas un second kind après le contrôle. Le modèle accepte uniquement caractères ASCII alphanumériques et `._-`, donc espaces, NUL et points Unicode ne deviennent pas des noms canoniques. La casse est conservée. `Store::lookup_decision` requiert le kind exact `security.decision`, dans un schéma sans collation NOCASE ; les voisins ou variantes de casse n'acquièrent pas l'autorité du cache. Des champs dans data/source/provenance ne remplacent pas ce kind externe.
- **PASS — Auth/rôles/tenant.** Le diff ne change ni authorize ni la sélection du Store. Authentification runner requise avant la garde ; source et provenance fournies ne peuvent autoriser un namespace interdit. Les restrictions viewer et tenant restent appliquées côté serveur.
- **PASS — Compatibilité.** Aucun blocage global de security.* ou usage.*. `usage.reported` reste une déclaration fournisseur, distincte d'une allocation centrale. Les 14 kinds de compatibilité couvrent les émissions existantes principales runner/runtime/workspace/agents/outils et le voisin sans autorité. La recherche des producteurs actuels ne révèle pas d'émission légitime par ingestion des racines maintenant réservées.
- **PASS — Erreurs et stockage.** Le refus est statique, sans réimprimer le contenu ou un secret. Il précède toute persistance ; aucun schéma, audit, contrat de décision, mécanisme de masquage, quota ou dépendance n'est modifié. Les autorisations légitimes passent toujours par la vraie route decide.

## Preuves examinées

Le nouveau test d'exploitation attend l'ingestion 403, une vraie décision deny pour la commande interdite, et une seule décision légitime dans la base. Le test de namespaces couvre les deux alias avec une provenance provider/declared. Le test de compatibilité conserve 14 traces admises. Leurs assertions ont été relues, sans les présenter comme une preuve de moteur.

Le journal `/tmp/ai-guard-p1-terminal-authority-tests.log` contient les résultats réels pertinents : decision_event_authority 3 PASS, decision_idempotence 9 PASS, api_security 4 PASS, tenant_api 3 PASS, jev_contract 2 PASS — **21 tests, zéro échec/ignoré** sur ces suites. Les exécutions appartiennent à l'auteur/orchestrateur, pas au reviewer. `/tmp/ai-guard-p1-next-clippy.log` termine normalement ; le rapport auteur conserve aussi formatage et RED.

Le journal antérieur `/tmp/ai-guard-p1-retention-authority-targeted.log` montre les trois tests API PASS puis un RED runtime_retention d'un autre lot ; il n'est pas utilisé comme preuve de succès global. Aucun test produit ni nouveau probe cible n'a été exécuté par le reviewer pour cette revue de source. Aucun audit complet du dépôt ou de sa couverture n'est revendiqué.

Empreintes finales relues :

```text
ed0cf280f609445fc30113d186928da312eac06f32fa9073701513a217b5a23f  src/api.rs
2ba9af95bb6d7dc25a97d44c0c146b42926fd3bee3c85446e10a059f869303ab  tests/decision_event_authority.rs
```

## Historique et limites

**L'ancien risque ne peut pas être déclaré assaini.** Une décision forgée avant ce refus peut avoir exactement le même contenu qu'une décision réellement émise par decide. Le diff ne migre, supprime ou réauthentifie aucune ligne. Le cache historique peut encore relire cette donnée ; la provenance centrale d'un ancien événement n'est pas prouvée. Le rapport auteur et `docs/api.md:24` l'indiquent explicitement. Une admission future doit obtenir une nouvelle autorisation centrale complète et ne pas convertir un ancien événement en droit start.

Le registre, le genesis/admission versionnés, la consommation durable et la protection de l'historique restent des obligations ouvertes. L'expression « v2 » du rapport auteur se comprend comme version de schéma/admission, pas comme permission de reporter une exigence obligatoire.

- PASS : revue ciblée de la garde et de ses consommateurs directs, tests/logs pertinents concordants, documentation de la limite historique.
- FAIL : aucun nouveau défaut confirmé dans ce diff ; le RED initial reste conservé comme résultat historique.
- NOT_RUN par le reviewer : nouveaux probes, suites globales, builds, moteur ou qualification de plateforme.
- BLOCKED pour qualification globale : authenticité des anciennes décisions et admission/consommation durable encore à traiter ; aucune clôture de ces obligations n'est déduite du correctif.
- NOT_APPLICABLE : cloud, publication, secrets réels et facturation pour cette revue.
