# Raccordement du bootstrap retenu au superviseur

Proposition initiale sur `552f103a`. Le 25 novembre, l’arbitrage
retient le broker comme propriétaire de la libération et précise la
garde commune avec STOP au point de soumission. Le raccordement et sa
qualification restent ouverts.
[Décision G05/E3](DECISIONS.md#25-novembre-2026--g05e3-libération-du-fournisseur-par-le-broker)

Le broker Rust de confiance possède le canal et l’autorité de libération pour une tentative donnée. Le bootstrap est retenu avant toute exécution du fournisseur. L’état central `Running` désigne le conteneur inspecté et le canal prêt, avec le fournisseur encore bloqué. Le broker attend ensuite une preuve d’ouverture durable fraîche pour la même tentative et vérifie que son autorité est toujours valide.

Le broker inscrit le marqueur durable avant le signal unique. STOP, perte d’autorité ou doute sur la livraison conduisent à l’arrêt ou à la réconciliation du même conteneur. L’échéance originale reste applicable. Le reçu historique SQLite reste rejouable et n’autorise aucun signal.

## Raccordement Docker et Podman à vérifier

Le broker prépare le canal avant de demander l’acquittement `Running`.
La poignée de préparation doit établir que le conteneur inspecté et
le canal appartiennent à la tentative courante. Le broker poursuit
ensuite l’ouverture durable et le traitement du reçu de cet appel.
La disponibilité de la boucle locale du helper indique seulement que
le helper écoute. Elle ne prouve pas que le bootstrap a reçu le signal.

Le transport retenu pour le premier raccordement local est le stdin
sans TTY du PID 1, détenu exclusivement par le broker pour cette
tentative. La garde d’invalidation couvre uniquement la vérification
finale et la soumission synchrone non bloquante de la petite trame.
L’attente de disponibilité et les requêtes d’inspection ou de contrôle
s’effectuent hors de cette garde. Le bootstrap valide la trame entière
avant de lire ses credentials ou d’appeler le fournisseur, y compris
pour une sonde de capacité ou une connexion au compte.

Une écriture complète prouve la soumission au transport du moteur.
L’observation du démarrage du fournisseur demande une preuve séparée.
Une soumission ambiguë ferme le droit d’écriture pour cette tentative
et déclenche la réconciliation avec l’échéance d’origine. Le parcours
TTY ne partage pas ce stdin et conserve son propre périmètre.
[Prévol local du transport](VALIDATION.md#25-novembre-2026--prévol-du-transport-stdin-e3)

## Preuve hors ligne

Exercer le point de composition réel en tenant l’acquittement de la première ouverture. Observer zéro signal avant l’ACK, puis un seul après l’ACK frais avec la même autorité. Vérifier zéro signal quand la tête avance vers STOP, que l’autorité est perdue ou que le reçu est historique. Une réponse ambiguë ne déclenche aucun second envoi. Les fixtures restent locales et le réseau reste coupé.
