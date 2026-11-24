# G09 · Motif du refus de réservation Legacy fresh

Base examinée `a7dbb86965f81252ef3b114d346127e22f11a99c`. Le reçu physique sur `c7f6c966` établit un refus `not_ready` lors de `reserve_prospective`, avec un Arm `pending` et le mode `fresh`.

## Frontière de compatibilité

Le [serveur Legacy](../../src/broker/enforcer/server_prospective.rs#L390) connaît le site du refus à l'instant du contrôle. Avant l'écriture de la réponse, [le serveur IPC](../../src/broker/enforcer/server.rs#L340) réduit ce résultat à `Response::Error { code }`. Le [format Legacy](../../src/broker/enforcer.rs#L739) rejette les champs inconnus et le décodeur d'`EnforcerError` rejette les valeurs inconnues. Ajouter un champ même facultatif au nouveau décodeur ferait échouer l'ancien décodeur dès réception de ce champ. Le [client](../../src/broker/enforcer/client_legacy.rs#L159) reçoit uniquement le code. La [projection du reçu](../../tests/support/broker_runtime.rs#L1067) ne peut donc pas retrouver le site du refus.

Le [lancement du gardien](../../src/broker/enforcer/client.rs#L2912) dirige stderr vers une sortie nulle. Une ligne locale au gardien ne parviendrait pas au reçu actuel. Un fichier partagé ou une lecture ultérieure d'un état mutable nécessiterait un contrat de publication et de corrélation. Un motif statique seul ne suffit pas à attribuer une écriture séparée à sa requête sous concurrence.

## Frontière d'un futur test déterministe

Le motif doit être choisi sur la branche qui refuse, puis rester associé à la réponse de cette tentative jusqu'à la projection. Une extension compatible avec les anciens pairs demanderait un mécanisme explicite de négociation ou un nouveau contrat de diagnostic privé corrélé. Les requêtes Legacy actuelles doivent continuer à recevoir exactement leur réponse existante. Cette évolution toucherait le gardien, le transport client et le consommateur du reçu. Le lot s'arrête à cette frontière.

Tester une réservation Legacy fresh à travers la réponse de sa tentative, avec une horloge injectée et des barrières de test pour l'installation et le STOP. Vérifier séparément les cas suivants.

| Contrôle déclenché | Oracle |
| --- | --- |
| Horloge rejetée avant allocation | Motif statique distinct, code `not_ready`, aucune réservation accordée |
| Horloge rejetée au contrôle avant ACK | Motif statique distinct, code `not_ready`, aucune réservation accordée |
| Runtime indisponible à l'expiration du délai d'installation | Motif statique distinct, code `not_ready`, aucune réservation accordée |
| STOP déjà verrouillé avec une horloge valide au contrôle avant ACK | Motif statique distinct, code `not_ready`, aucune réservation accordée |

Une cellule peut rester retenue après allocation sans qu'un grant ait été remis. Vérifier aussi le succès nominal et le refus Legacy historique avec des pairs de versions différentes. La projection doit exposer uniquement les codes fermés. Conserver l'ordre des contrôles, leurs délais, les refus, le verrou STOP et les conditions de l'ACK.
