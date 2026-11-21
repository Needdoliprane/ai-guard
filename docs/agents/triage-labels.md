# États de triage

Inscrire la valeur dans la ligne `Status:` du [ticket local](issue-tracker.md).

| Valeur | Signification |
|---|---|
| `needs-triage` | Le mainteneur doit qualifier la demande |
| `needs-info` | Il manque une information pour décider |
| `ready-for-agent` | Le travail est détaillé et disponible pour un agent |
| `ready-for-human` | Le travail demande une intervention humaine |
| `wontfix` | La demande a été écartée |

Les verdicts `PASS`, `FAIL`, `BLOCKED` et `NOT_RUN` s'appliquent aux preuves de validation. Un ticket prêt pour un agent peut encore attendre une dépendance indiquée dans `Blocked by:`.

Lors du triage, renseigner aussi `Category: bug` pour un défaut ou `Category: enhancement` pour une évolution.
