# Raccordement du bootstrap retenu au superviseur

Proposition à arbitrer sur `552f103a`. Le contrat E3 décrit l’ordre des preuves et laisse ouvert le propriétaire de la libération.

Le broker Rust de confiance possède le canal et l’autorité de libération pour une tentative donnée. Le bootstrap est retenu avant toute exécution du fournisseur. L’état central `Running` désigne le conteneur inspecté et le canal prêt, avec le fournisseur encore bloqué. Le broker attend ensuite une preuve d’ouverture durable fraîche pour la même tentative et vérifie que son autorité est toujours valide.

Le broker inscrit le marqueur durable avant le signal unique. STOP, perte d’autorité ou doute sur la livraison conduisent à l’arrêt ou à la réconciliation du même conteneur. L’échéance originale reste applicable. Le reçu historique SQLite reste rejouable et n’autorise aucun signal.

## Preuve hors ligne

Exercer le point de composition réel en tenant l’acquittement de la première ouverture. Observer zéro signal avant l’ACK, puis un seul après l’ACK frais avec la même autorité. Vérifier zéro signal quand la tête avance vers STOP, que l’autorité est perdue ou que le reçu est historique. Une réponse ambiguë ne déclenche aucun second envoi. Les fixtures restent locales et le réseau reste coupé.
