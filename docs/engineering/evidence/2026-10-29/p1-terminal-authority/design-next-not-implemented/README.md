# DESIGN — prochain registre, non implémenté et non qualifié

Archives de conception issues de `/tmp/`, distinctes des preuves d'implémentation. Les DTO, routes workers, tables/schema v2, reçus d'admission, quotas, start-claim, observations, allocation et primitives de reprise proposés ici **ne sont pas des API ou fonctions livrées**. Aucun test futur listé dans ces documents n'est déclaré exécuté.

- [Synthèse core](worker-core-contract.md), source `/tmp/ai-guard-worker-core-contract.md` : admission fraîche liant LaunchSpec/reçu, transactions tenant/audit/quota, consommation de start non rejouable et ledger d'allocation `billable=false`.
- [Détail core](worker-core-contract-detail.md), source `/tmp/ai-guard-worker-core-contract-detail.md` : DTO/SQL/migration proposés, contrôle de projections, matrice de tests et paramètres encore à figer.
- [Contrat runtime suivant](runtime-contract-next.md), source `/tmp/ai-guard-runtime-contract-next.md` : opération stable, autorité moteur, identité d'instance, observation et verrou par tentative mono-hôte ; aucune lease présentée comme fencing multi-hôtes.

Le document runtime contient aussi une description du petit lot P1 retained, désormais couvert par ses preuves séparées. Ses formulations antérieures sur cleanup/retained ne remplacent pas le contrat final `UNCONFIRMED` et `error_audit`. Les liens absolus `/tmp/` et descriptions du checkout dans les trois archives conservent leur contexte de rédaction ; les [preuves du lot réalisé](../README.md) et l'état courant font autorité.

Ces conceptions prolongent l'architecture existante Rust/SQLite par client ; elles ne constituent ni implémentation de registre, ni qualification 100 workers, ni exactement une fois moteur, ni Kubernetes/cloud, prix ou facture. La mention v2 désigne une proposition de version de schéma/admission, sans report des obligations du produit. Les arbitrages restant listés doivent être traités dans le prochain lot avec critères et preuves propres.
