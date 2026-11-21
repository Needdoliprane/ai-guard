# Suivi local des tickets

Les workflows des skills utilisent des fichiers Markdown sous `.scratch/`, ignoré par Git. Le [plan](../engineering/IMPLEMENTATION_PLAN.md) et le [cahier de tests](../engineering/TEST_STRATEGY.md) restent les références communes pour suivre les chantiers et leurs preuves.

- Une spec par lot dans `.scratch/<lot>/spec.md`.
- Un fichier par ticket dans `.scratch/<lot>/issues/<NN>-<sujet>.md`, numéroté à partir de `01`. Une ligne `Status:` porte l'[état de triage](triage-labels.md) et `Blocked by:` liste les numéros des dépendances.
- Ajouter les échanges du ticket sous `## Comments` et conserver les éléments utiles à sa reprise, notamment le périmètre du lot, les fichiers possédés et le critère d'acceptation.
- Pour `wayfinder`, tenir la carte dans `.scratch/<lot>/map.md`. Les tickets de décision ont une ligne `Type:` et passent à `Status: claimed` puis `Status: resolved`. Reporter la conclusion sous `## Answer` et la relier dans la carte.

Dans ces workflows, « publier dans le suivi » veut dire écrire ce fichier local. Pour reprendre un ticket, lire son chemin ou le numéro dans le lot concerné. Le lot garde ses preuves dans l'emplacement prévu par la stratégie de test.
