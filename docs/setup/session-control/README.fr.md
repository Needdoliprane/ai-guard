# Préparer une session et envoyer une consigne

[English](README.md)

Utilisez un dashboard et un gateway configurés en `owned_sources`. Connectez-vous avec un compte nominatif et sélectionnez un contexte personnel. Demandez à l’opérateur l’identifiant opaque de la cible de lancement et sa révision courante. Le contexte doit autoriser chaque action séparément. [Sources owned](../identity/OWNED_SOURCES.fr.md)

1. Ouvrez **Nouvelle session IA**, saisissez la cible et la révision publiées, puis choisissez **Préparer le lancement**. Vérifiez la préparation retenue, son expiration et son reçu d’audit. La préparation laisse le worker et le quota intacts.
2. Choisissez **Lancer la session** pour valider cette préparation. Le reçu d’acceptation identifie la session et son grant nominatif. Consultez les observations du broker et du runtime pour établir l’admission et le démarrage.
3. Si le résultat est inconnu, utilisez **Réessayer le même lancement** depuis le même contexte. Cette action reprend l’opération et la préparation déjà soumises. Le contrôle peut restituer un reçu existant après expiration de la préparation. Un refus pendant la réconciliation laisse l’issue initiale inconnue.
4. **Envoyer une consigne** exige une liaison de conversation courante pour la session, le worker, la tentative et l’époque d’entrée exacts. Vérifiez la destination avant l’envoi. Relisez le reçu pour distinguer enregistrement, transmission, acquittement et livraison inconnue. L’acquittement porte sur le protocole du fournisseur. Observez séparément les actions qui en découlent.

L’implémentation actuelle couvre les opérations durables du plan de contrôle et les vues du dashboard. Les tests du composeur du broker utilisent un enfant stdio contrôlé et un vrai Store owned. Le raccord à l’admission supervisée de production et les parcours complets avec runtimes et fournisseurs restent ouverts. [Contrat de session](../../engineering/SESSION_CONTROL.fr.md) · [Gates de qualification](../../engineering/PRODUCTION_READINESS.md)
