# G04 — Lier l’initiateur nominatif à l’admission supervisée

**Proposition soumise au mainteneur.** Base `38341fb1`. Périmètre de décision limité au passage d’un lancement humain préparé vers une admission durable. Premier oracle sur SQLite, sans fournisseur, avant tout claim de démarrage.

## Articulation avec Prepare

Le [contrat de session](SESSION_CONTROL.fr.md) fixe déjà la cible opaque, la préparation retenue, la confirmation d’une empreinte exacte, le grant nominatif de session et la distinction entre acceptation, ordre de démarrage et exécution observée. La proposition s’inscrit dans le même lot de contrôle de session. Elle réutilise son identifiant d’opération et sa préparation, sans second catalogue ni parcours utilisateur concurrent.

| Référence examinée | Point de raccord |
|---|---|
| `3a36d554` | Le commit ajoute la consultation de la chaîne d’attribution sur demande. Sa branche décrit le parcours Prepare/Lancer sur sources Legacy et le refus des routes de session en mode owned. La vue consomme les faits établis par le contrôle. [Guide au commit](https://github.com/Needdoliprane/ai-guard/blob/3a36d554/docs/setup/session-control/README.fr.md) |
| `f294c786` | Le consommateur broker passe par le Submit privé existant. Le correctif rattache les faits de démarrage aux observations concordantes de la même opération, du worker et de la tentative. L’acceptation de l’admission et ces observations restent des preuves distinctes. [Correctif](https://github.com/Needdoliprane/ai-guard/commit/f294c786) |
| [Capacités de contrôle](CONTROL_CAPABILITIES_PROPOSAL.md) | Réutiliser la négociation privée pour déclarer une prise en charge effective si le rail en a besoin. Une capacité annoncée renseigne la compatibilité du transport et du Store. L’autorisation reste vérifiée à l’admission. |

Les deux commits Prepare examinés sont hors de l’ascendance de `38341fb1`. Leur composition reste à vérifier avant implémentation. Le conducteur Keycloak `3a0d166c` prouve une lecture personnelle auditée sur `ca3c161b`. Le plafond OIDC et l’appartenance DEMO de `user-1` portent lecture et export. Pour l’oracle positif, provisionner explicitement le droit de lancement dans une fixture contrôlée. [Conducteur](https://github.com/Needdoliprane/ai-guard/blob/3a0d166c/.github/workflows/g04-keycloak-native.yaml) · [Manifeste DEMO](https://github.com/Needdoliprane/ai-guard/blob/ca3c161b/docs/setup/sso/examples/local-demo.json)

## Autorité proposée

1. **Le gateway émet la preuve.** Après confirmation humaine de l’empreinte Prepare, il vérifie la session authentifiée et le contexte personnel. Il inscrit la liaison dans le Store du tenant avec le principal canonique vérifié (issuer et sujet OIDC ou identité locale), l’organisation, le tenant et le namespace, l’opération de lancement, l’empreinte de préparation et de la demande, la cible et sa révision, le broker et son profil autorisés, ainsi qu’une échéance. Il conserve les références des versions d’autorisation et, si nécessaire, de l’approbation nominative de cette demande exacte. Le nom affiché et les champs libres du client restent hors de cette décision.
2. **Le broker présente une référence à cette liaison** via le canal de contrôle authentifié, avec l’opération et le contenu exacts à admettre. La référence désigne un enregistrement durable, accessible au gateway du tenant. Elle ne confère aucun droit à son porteur. Le broker est l’acteur effectif de l’acceptation et le principal humain reste l’initiateur. Le credential runner reste entre broker de confiance et gateway. Le jeton humain reste dans le circuit d’authentification humain. Aucun des deux ne rejoint le worker.
3. **Le gateway et le Store valident au point de commit.** Le gateway authentifie le broker sur le rail configuré. Le Store résout la liaison dans le tenant autorisé et vérifie opération, broker, namespace, échéance, empreintes et révisions. L’origine d’une demande humaine découle du fait durable établi par le gateway. Une demande liée à Prepare dont la référence manque ou diverge est refusée, y compris lors d’une reprise. Le chemin de soumission manuelle reste distinct et conserve sa propre provenance.

La proposition retient une référence résolue côté serveur avec une liaison durable, sans besoin de distribuer une preuve signée autonome au broker. La comparaison porte sur la demande canonique complète, y compris la résolution du fournisseur et les limites de lancement pertinentes.

## Transaction, reprise et révocation

- Le marqueur de droits humains, l’appartenance active, le plafond OIDC, `launch_session`, le périmètre de la cible, son ACL, l’état de révocation et l’éventuelle approbation doivent encore autoriser l’effet. Le contrôle réutilise les autorités de contexte et de session existantes. Toute vérification réalisée en amont doit être reliée à une version ou un état de révocation que la transaction compare à l’état courant. Si une source faisant autorité ne fournit aucun moyen de valider cet état au point de commit, l’admission humaine reste fermée sur ce rail.
- Le Store sérialise la validation finale avec la révocation et les mises à jour concurrentes du quota. Sur succès, il enregistre atomiquement la consommation de la liaison, le grant nominatif de session, l’admission supervisée, la réservation de quota et le reçu d’audit obligatoire qui les relie à la décision. La décision peut préexister avec son reçu durable et son empreinte exacte. Toute erreur de décision, d’audit ou de commit bloque la publication d’un succès et tout démarrage fondé sur ce succès.
- Même opération et empreinte, avec droit de consultation courant, retrouvent le reçu déjà commis sans nouveau quota, nouveau droit de démarrer ni nouvel appel de politique. Même opération avec contenu ou liaison divergents produit un conflit. Après perte d’ACK ou commit incertain, réconcilier la même opération depuis le Store avant toute nouvelle tentative. Une reprise technique du broker peut réconcilier l’état avec son identité propre sans restituer le reçu à un humain révoqué.
- Une révocation ou une expiration gagnant avant l’admission la bloque. Après admission et avant claim, la révocation invalide le lancement encore retenu. Le contrôle clôt l’admission et libère le quota par la transition auditée existante, après avoir exclu un claim concurrent. Si le claim a déjà gagné, appliquer arrêt ou réconciliation selon le contrat broker et conserver les faits historiques. Le quota suit l’état durable et l’effet effectivement observé.

## Rails Legacy et owned

| Rail | Proposition |
|---|---|
| Legacy SQLite | Première composition avec le contrôle de session Legacy et l’admission supervisée privée. Réutiliser Prepare et le Store transactionnel. Provisionner le marqueur et le droit nominatif selon le parcours de maintenance existant. Le raccord de la liaison nécessite un contrat privé explicitement versionné. Préserver les requêtes historiques de soumission manuelle. |
| Owned | Conserver le refus de session tant qu’un profil de mutation ne prend pas explicitement en charge cette admission. Les routes de lecture et les sept opérations de présence/réconciliation existantes conservent leur périmètre. Une adoption ultérieure doit lier la preuve au tenant, au namespace et à l’autorité broker de la source owned, et prouver la même transaction et le même refus après révocation. Un échec sur owned reste un refus sur ce rail. |

Les variantes MySQL, l’exécution physique et les parcours fournisseur gardent leurs preuves propres. Voir le [contrat d’architecture](ARCHITECTURE_CONTRACT.md) et la [stratégie de tests](TEST_STRATEGY.md).

## Oracle de preuve à la frontière d’admission

Exercer le routeur Rust en mémoire, le vrai Store de recette et le contrat privé du broker. Utiliser une preuve humaine de test résolue vers l’issuer et le sujet DEMO de `user-1`, une cible opaque de commande fixe et une politique Rust déterministe. Observer le reçu et l’état durable après réouverture. Le cas nominal s’arrête au reçu. Le cas de révocation après admission vérifie uniquement le refus d’un claim au contrôle. Aucun processus métier ni appel fournisseur n’est nécessaire.

| Cas | Oracle attendu |
|---|---|
| DEMO sans droit, puis droit provisionné | Premier passage refusé sans admission ni quota. Avec droit et Prepare confirmé, un seul reçu relie la demande exacte, le principal authentifié, la décision, le broker, le grant et la réservation. |
| Sujet libre, autre tenant ou broker, empreinte modifiée | Refus avant admission et quota. Aucun reçu d’un autre périmètre divulgué. |
| Révocation ou expiration avant commit | Refus sans réservation. Une course ordonnée dans les deux sens démontre l’ordre durable. Après commit et avant claim, la révocation empêche le claim et la transition auditée régularise le quota. |
| Réponse perdue et rejeu concurrent | Même reçu après réouverture, une seule réservation. Divergence en conflit. Le contexte révoqué perd l’accès au résultat. |
| Quota plein, échec d’audit ou commit incertain | Aucun succès sans reçu durable. Après réconciliation, soit l’admission complète existe une fois, soit aucune réservation résiduelle n’existe. |
| Rail owned sans profil de mutation | Refus avant effet, sans bascule vers Legacy. |

Cet oracle établira l’attribution et l’atomicité à l’admission. Le parcours Keycloak réel avec action d’agent et l’observation du runtime restent à qualifier. **Oracle proposé, non exécuté.**

## Décision attendue

Valider l’émission par le gateway d’une liaison durable référencée par le broker, la revérification des droits dans la transaction d’admission et l’ordre de livraison Legacy SQLite puis owned après adoption explicite. Confirmer que le raccord appartient au lot Prepare existant. La validation de cette proposition autoriserait à spécifier le contrat privé et ses tests, sans déclarer de fermeture de G04 ou de G07.
