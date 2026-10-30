# Revue indépendante — dernier delta runtime_smoke

Date : 2026-10-29. Lecture seule de `tests/runtime_smoke.rs`, diff recalculé contre `/tmp/ai-guard-decision-baseline.woj2zP/tests/runtime_smoke.rs` et comparaison avec le sous-lot loopback déjà revu. Aucun moteur/HTTP relancé, aucune édition du dépôt, aucun secret lu ni ressource supprimée.

**PASS pour ce delta de test et la preuve ciblée examinée. Aucun défaut bloquant identifié.**

- Le `touch` refusé cible uniquement un marqueur UUID dans la source QA fraîche et conservée (`tests/runtime_smoke.rs:116`). Un repli hôte exécutant cette commande laisserait un fichier observable et sans effet sur un projet utilisateur. Le test exige une erreur, puis exactement une décision d'audit `deny` liée au marqueur (`:124`). Il ne transforme pas une erreur quelconque du runner en preuve de refus.
- L'absence de conteneur est interrogée avec le label de la session extraite de cette décision (`:137`). Une erreur du CLI échoue via `unwrap`, une réponse tronquée échoue explicitement ; la sortie doit être vide.
- L'inventaire non filtré des volumes doit être réussi et non tronqué, contenir le volume du premier vrai lancement et ne pas contenir celui du refus (`:158`). Ce contrôle positif dans la même réponse évite qu'une panne ou une sortie vide soit prise pour une absence. Le marqueur doit également rester absent de la source (`:175`).
- Source et parent d'export passent à `TempDir::keep` dès leur création (`:71`, `:182`), avec chemins imprimés avant les opérations suivantes. Aucun cleanup destructeur nouveau : volumes et preuves conservés, nettoyage normal des seuls conteneurs d'opération inchangé. Les overrides et la validation loopback restent ceux déjà revus.

| Statut | Preuve examinée | Résultat |
| --- | --- | --- |
| PASS | `/tmp/ai-guard-real-smoke-final.log` | 1 test réel réussi, 0 échec, 0 ignoré, 3.25 s ; 1 test logiciel filtré. |
| PASS | Sessions du log | Exécution `ab79d25c-946a-465c-a4df-8fe7a7bef099`, mutation `b3c54091-1b1e-4e43-ac23-27e53737b034`, refus `1c7c3dcf-1eb2-4398-9232-45f8c438c6f0`. |
| PASS | Inventaire des deux répertoires de preuve, relu sans contenu de clé | Source `.tmploHnqA` : `.env` de fixture et `original.txt` ; export `.tmpPe38RH/export` : `generated.txt` et `original.txt`. Aucun marqueur de refus listé. |
| PASS | Assertions inchangées dans le test | UID 1000 dans la complétion auditée de la bonne session, mutation isolée, export neuf sans `.env`, texte original inchangé. |
| NOT_RUN dans ce smoke | Trois workers, frontières tenant, relais/usage, pannes, autres runtimes/plateformes | Aucune qualification déduite de ces trois demandes successives. |

L'inventaire prouve l'absence **au moment du contrôle**, pas une trace exhaustive de chaque tentative transitoire possible. Le marqueur vérifie le repli hôte de cette commande précise. Le succès ne prouve pas tous les refus ni toutes les pannes d'audit ; le source runtime dispose de sa revue distincte. Le mode rootful explicitement autorisé reste visible dans le log ; aucune qualification rootless n'est affirmée.

Empreinte recalculée : `tests/runtime_smoke.rs` **`f271838fcd0718751f35223bda57a0c2c18c39ec60338ff436782c4334d9dde3`**. RTK et contrats architecture/coding/tests relus. Aucun blocage restant propre à ce delta ; aucune qualification production globale revendiquée.
