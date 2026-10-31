# Identité individuelle, SSO et essai rapide

Décision du mainteneur, 31 octobre 2026. **Cible approuvée, à implémenter et qualifier.** Ce document décrit les fonctionnalités à livrer ; il n'annonce aucune commande SSO ou stack Keycloak actuellement disponible. Le [quick start existant](quickstart.md) reste le parcours exécutable documenté. Le suivi est dans le [plan d'implémentation](engineering/IMPLEMENTATION_PLAN.md).

## Essayer sans configurer toute une infrastructure

L'objectif est qu'une personne découvrant ai-guard puisse démarrer la pile avec une commande Compose documentée, fournir une seule clé API du fournisseur choisi, ouvrir le dashboard, exécuter une première tâche non destructive et retrouver son audit. Aucun compte cloud ai-guard ni clé pour les autres fournisseurs n'est requis. Les modes abonnement restent disponibles selon leurs prérequis propres, sans basculement silencieux vers une API payante.

**Environ cinq minutes est une cible d'expérience, pas une promesse.** Mesurer le parcours sur une machine et un moteur identifiés, avec durée jusqu'à interface disponible puis jusqu'au premier résultat audité. Publier séparément téléchargement/construction initiale, démarrage avec images présentes, étapes manuelles et erreurs rencontrées. Préférer des artefacts prêts à utiliser et versionnés ; ne pas réduire les contrôles de sécurité pour gagner du temps.

La procédure doit fournir les prérequis et leur diagnostic, le lancement exact réellement essayé, les accès locaux, la saisie privée de la clé, une commande CLI utile, l'examen des résultats, l'arrêt et la reprise. Elle préserve les installations existantes. Une compilation Rust ou une configuration manuelle ne doit pas être présentée comme inutile si le chemin livré l'exige encore.

## SSO facultatif et fournisseur interchangeable

ai-guard intègre un fournisseur d'identité ; il ne devient pas un fournisseur SSO. Le SSO reste facultatif à l'installation. Un mode local nominatif, utilisable hors ligne, conserve attribution, permissions et audit ; désactiver le SSO ne désactive pas l'authentification.

Le connecteur utilise OpenID Connect, Authorization Code et PKCE S256 avec clients publics pour le dashboard et le CLI. Aucun `client_secret` n'est embarqué dans le build ou distribué avec la configuration. `issuer`, identifiants clients et redirections sont des paramètres publics configurables au déploiement ; le vérificateur PKCE et les jetons restent privés. La configuration et les droits sont validés côté Rust, avec refus des fournisseurs/redirections non approuvés. Un ID Token n'est pas automatiquement un jeton d'accès aux API ai-guard.

L'identité locale durable représente le couple OIDC vérifié `(iss, sub)` ou une identité locale nominative. L'adresse e-mail sert au profil, pas de clé d'attribution. Les appartenances aux tenants et les rôles ai-guard sont contrôlés côté serveur. L'administration du fournisseur n'accorde pas implicitement le rôle de contrôle `runner`. Les droits humains permettent de demander les actions autorisées au broker de confiance ; aucun credential `runner` n'est distribué au navigateur ou aux workers.

La documentation de raccordement entreprise doit préciser : fournisseur approuvé et découverte OIDC, clients Web/CLI et redirections exactes, PKCE, validation des jetons, correspondance utilisateurs/groupes/rôles/tenants, sessions et révocation, rotation des clés, diagnostic et retrait de la configuration. L'authentification ai-guard reste indépendante des abonnements et clés API Codex/Claude.

## Profil Keycloak de découverte

Fournir un profil Compose explicitement activé, avec version de Keycloak épinglée et configuration de realm importée automatiquement. Keycloak est un service tiers de démonstration, sans ajout de logique métier ai-guard dans un autre langage. Le nom des fichiers, les ports et les commandes seront fixés par l'implémentation vérifiée ; aucun exemple de nom donné en conversation n'est un contrat existant.

| Compte préconfiguré | Identifiants publics de démonstration | Usage |
|---|---|---|
| Administrateur Keycloak | `admin / admin` | Administrer Keycloak et le realm de démonstration |
| Utilisateur applicatif | `user / user` | Se connecter au dashboard et au CLI ai-guard, avec les permissions applicatives de l'essai |

Les clients publics, PKCE S256, redirections et permissions nécessaires au parcours sont prêts à l'emploi ; aucune édition de realm ni intervention dans la console d'administration n'est nécessaire au premier essai. Les accès hôte de cette démo sont limités au loopback. Les identifiants connus ne sont jamais appliqués aux installations normales ni aux credentials existants. Aucun jeton SSO humain, credential d'administration Keycloak ou secret du contrôle ne rejoint un worker.

Le parcours d'acceptation démarre la pile depuis un état neuf, ouvre le dashboard, connecte `user`, connecte le CLI au même compte, lance une tâche réelle non destructive, puis retrouve demande, exécution et résultat dans l'audit. Un contrôle vérifie aussi la séparation entre administration Keycloak et autorisations ai-guard. Les comptes et le realm sont explicitement nommés « démonstration » ; aucune activité d'agent fictive n'est injectée dans le produit.

Cette configuration valide l'interopérabilité et facilite l'évaluation du produit. L'exploitation d'un Keycloak de production appartient à l'opérateur ; ai-guard fournit une procédure de raccordement et n'impose pas ce fournisseur aux clients.

## Chaîne de traçabilité à livrer

La chaîne attendue est : **utilisateur initiateur → demande → session → agent → délégation → sous-agent → action/commande → décision → acceptation broker → observation runtime → résultat et modifications**.

- Le contrôle établit les identités et les liens parent/enfant. Chaque agent/sous-agent vérifié possède une identité d'exécution bornée, expirante et révocable, avec des permissions au plus égales à celles déléguées. Un nom dans un log ne suffit pas.
- Chaque événement distingue initiateur, acteur effectif, origine et parents causaux. Conserver les demandes/réponses visibles et les apports externes accessibles à l'intégration ; une action issue d'un contenu externe ne devient pas une instruction humaine explicite. Le raisonnement interne non exposé par un fournisseur ne peut pas être promis comme observable.
- Les commandes conservent arguments et contexte structurés. Demande d'outil, décision et version du contrôle, approbation, acceptation, exécution observée, sorties et changements de fichiers sont des faits distincts, liés sans réécrire l'historique.
- L'audit est durable et indépendant des workers, avec chaînage, points de contrôle signés et export vérifiable. Définir la protection des clés, la portée de la vérification et les limites face à un administrateur du stockage ; une signature ne prouve pas un effet runtime absent des observations.
- Les contenus sensibles ont des accès restreints, une protection adaptée et une rétention configurable. Masquer les secrets avant persistance/diffusion ; borner les volumes. Afficher explicitement masquage, troncature, données perdues et couverture partielle. Une chaîne incomplète n'est jamais certifiée complète.
- La consultation permet de remonter d'une commande à la demande initiale et de parcourir les délégations, décisions et résultats, selon les droits du tenant. Un audit obligatoire indisponible bloque l'action ; une déconnexion ne fabrique pas un résultat vide ou réussi.

Prévoir migrations et compatibilité avec les événements et credentials existants. Une ancienne clé partagée attribue au mieux une activité à son périmètre authentifié ; ne pas inventer une personne pour les événements historiques.

## Qualification et documentation

Le [cahier de tests](engineering/TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier) couvre le mode sans SSO, une connexion réelle Keycloak Web/CLI, les refus, pannes et révocations, la séparation des tenants, la chaîne de délégation et l'essai chronométré. Un second fournisseur OIDC peut compléter l'interopérabilité sans remplacer la preuve Keycloak. Aucun faux fournisseur permanent ni scénario d'affichage ne qualifie ces parcours.

Mettre à jour ensemble quick start, exemple Compose/realm, configuration, aide CLI, exploitation, dépannage et preuves. Fournir les deux entrées « essayer avec la démo » et « raccorder le SSO de l'entreprise ». Le support SSO doit être qualifié dans le produit même si un déploiement peut le laisser désactivé. Les gates globaux restent ouverts tant que leurs preuves manquent.

## Références officielles

- [OpenID Connect Core](https://openid.net/specs/openid-connect-core-1_0.html) : identité, claims et validation.
- [OAuth pour applications natives, RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html) : navigateur externe et PKCE pour le CLI.
- [Sécurité OAuth, RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html#section-2.1.1) : Authorization Code et PKCE S256.
- [Clients publics Keycloak](https://www.keycloak.org/securing-apps/javascript-adapter) : configuration sans secret client et redirections précises ; cette référence n'impose pas le SDK JavaScript au produit.
- [Keycloak avec Docker](https://www.keycloak.org/getting-started/getting-started-docker) et [conteneurs](https://www.keycloak.org/server/containers) : environnement de découverte et configuration conteneurisée.
