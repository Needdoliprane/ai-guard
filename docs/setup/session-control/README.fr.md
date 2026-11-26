# Préparer une session et envoyer une consigne

[English](README.md)

Utilisez un dashboard et un gateway configurés en `owned_sources`. Connectez-vous avec un compte nominatif et sélectionnez un contexte personnel. Demandez à l’opérateur l’identifiant opaque de la cible de lancement et sa révision courante. Le contexte doit autoriser chaque action séparément. [Sources owned](../identity/OWNED_SOURCES.fr.md)

1. Ouvrez **Nouvelle session IA**, saisissez la cible et la révision publiées, puis choisissez **Préparer le lancement**. Vérifiez la préparation retenue, son expiration et son reçu d’audit. La préparation laisse le worker et le quota intacts.
2. Choisissez **Lancer la session** pour valider cette préparation. Le reçu d’acceptation identifie la session et son grant nominatif. Consultez les observations du broker et du runtime pour établir l’admission et le démarrage.
3. Si le résultat est inconnu, utilisez **Réessayer le même lancement** depuis le même contexte. Cette action reprend l’opération et la préparation déjà soumises. Le contrôle peut restituer un reçu existant après expiration de la préparation. Un refus pendant la réconciliation laisse l’issue initiale inconnue.
4. **Envoyer une consigne** exige une liaison de conversation courante pour la session, le worker, la tentative et l’époque d’entrée exacts. Vérifiez la destination avant l’envoi. Relisez le reçu pour distinguer enregistrement, transmission, acquittement et livraison inconnue. L’acquittement porte sur le protocole du fournisseur. Observez séparément les actions qui en découlent.

Pour retrouver une session existante, ouvrez **Ressources** et choisissez **Consulter le canal** sur une session dont vous êtes propriétaire. Le dashboard relit la liaison auprès du gateway. Choisissez **Utiliser ce canal** quand il est actif et que son échéance permet encore l’envoi. **Relire le canal** vérifie son état courant. Si l’époque d’entrée change, sélectionnez explicitement le nouveau canal.

Si votre contexte autorise l’envoi et ne propose pas la vue Ressources, ouvrez **Sessions** et saisissez l’identifiant exact d’une session dont vous êtes propriétaire. Le gateway vérifie le contexte, le tenant et la propriété de la session.

L’implémentation actuelle couvre les opérations durables du plan de contrôle et les vues du dashboard. Les tests du composeur du broker utilisent un enfant stdio contrôlé et un vrai Store owned. Le raccord à l’admission supervisée de production et les parcours complets avec runtimes et fournisseurs restent ouverts. [Contrat de session](../../engineering/SESSION_CONTROL.fr.md) · [Gates de qualification](../../engineering/PRODUCTION_READINESS.md)
