# Stockage SQL configurable

Le produit doit permettre trois profils explicites par tenant. SQLite reste le stockage local par défaut. Les deux profils MySQL distants utilisent un identifiant et un mot de passe. Le profil mTLS exige aussi un certificat client et sa clé. Tous les profils MySQL vérifient la chaîne et le nom du serveur.

Les identifiants et clés restent des références privées accessibles au gateway Rust. L’identité authentifiée détermine le tenant et son stockage. Le backend choisi reste le même en cas de panne. Une perte de l’audit durable bloque les actions qui en dépendent.

Le stockage SQLite utilise un fichier local au contrôle. Lors d’un changement de backend ou de destination, l’opérateur prépare la cible et réalise lui-même tout transfert d’historique. À partir des seuls accès à la cible configurée, ai-guard valide le schéma, le tenant et l’état durable, puis ajoute les nouveaux événements à la suite de l’historique présent. Le journal existant reste intact. La transaction met à jour la tête et les projections nécessaires à la nouvelle écriture.

Utiliser une CA autosignée dédiée pour la recette. Exécuter séparément les deux profils MySQL avec un vrai serveur et conserver les refus de mot de passe et de certificat distincts. Une connexion réussie au serveur est un prévol. La qualification ai-guard exige le parcours applicatif, les transactions et les régressions SQLite.

Lire le [contrat de stockage](../../docs/engineering/ARCHITECTURE_CONTRACT.md#stockage-sql-par-tenant) et le [cahier de tests SQL](../../docs/engineering/TEST_STRATEGY.md#stockage-sql-configurable--à-qualifier).
