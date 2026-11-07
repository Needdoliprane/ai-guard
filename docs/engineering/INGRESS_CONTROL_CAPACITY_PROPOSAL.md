# Capacité réservée au contrôle — proposition du lot H

Statut : **PROPOSÉ, décision du mainteneur requise avant implémentation**.
Source examinée : `647fb1b1a8305eb0ad79bc1f0fdf83afbaea5b18`, 7 novembre 2026.
Ce document ne livre aucun listener, réglage ou garantie de disponibilité.

## Besoin constaté

Le runner/broker rejoint actuellement le gateway interne par l'ingress publié
sur loopback. Le navigateur rejoint le dashboard par un autre listener du même
relais. Chaque listener possède ses propres pools de 16 uploads et 64 échanges,
mais tous les appels d'un listener partagent ces pools avant authentification.
Le correctif initial C a séparé uploads incomplets et échanges ; il ne réserve
pas de capacité aux mutations de contrôle avec corps.

Le rapport final run-1 et son témoin `audit_cmp_ingress_refusals.rs` montrent
qu'un client sans credential peut occuper ce budget public et faire refuser un
POST de contrôle avant son arrivée au gateway. Le rôle, le tenant et la
durabilité restent décidés dans Rust côté gateway ; le relais n'a aucun secret.
Voir le [suivi canonique](AUDIT_FOLLOWUP.md#audit-final-run-1).

## Choix proposés

1. **Canal de contrôle distinct terminé dans le gateway — recommandé.**
   Admission et concurrence propres, authentification avant corps et capacité
   d'opération ; l'ingestion déclarée reste sur le canal public. Conserver le
   relais sans secrets et les services internes. Un raccord passant par ce
   relais doit transmettre les en-têtes au gateway avant d'attendre le corps,
   plutôt que réutiliser son pipeline public de buffering et ses pools.
2. Ajouter seulement un port ou réserver selon chemin/header dans l'ingress.
   Cela déplace le budget anonyme sans établir d'identité ; insuffisant seul.
3. Distribuer au relais des credentials ou lui faire choisir le tenant.
   Cela change son rôle de confiance et duplique l'autorisation ; non retenu.

La direction 1 ajoute une frontière de transport dans les composants Rust
existants, sans nouveau service métier. Elle doit être approuvée avant code.
La mécanique HTTP exacte et la représentation de configuration feront l'objet
de leur contrat et revue avant intégration ; aucun nom d'option n'est livré ici.

## Contrat de la direction recommandée

- Le gateway vérifie l'identité de contrôle requise par la route avant de lire
  le corps et avant de réserver la capacité d'une opération : `RunnerIdentity`
  pour les mutations runner ; l'actuel `VerifiedBroker` uniquement pour ses
  routes de présence liées à son grant. Aucune conversion broker → runner.
  Anonyme, viewer et worker
  n'obtiennent aucun slot réservé. Le canal utilise les mêmes autorisations,
  transactions, tenants et refus d'audit que les handlers existants.
- Décisions, mutations, claims, présence et callbacks de confiance choisissent
  explicitement le transport contrôle. La capture déclarée utilise l'ingestion
  publique ; `Event.kind`, `source` ou une assertion du worker ne changent jamais
  le canal d'autorité. Aucun fallback après refus, panne ou saturation.
- Le canal n'est ni un proxy arbitraire, ni un accès moteur, ni un flux SSE
  public. Corps, en-têtes, concurrence et délais restent bornés. Une requête
  invalide ou trop volumineuse n'est jamais partiellement appliquée.
- Les destinations du relais sont fixes, configurées par le contrôle. Il ne
  possède aucun credential propre, monté ou persisté, aucune donnée persistante
  et aucun socket de moteur. L'en-tête d'authentification transite nécessairement
  vers le gateway : le relais ne l'interprète, ne le conserve et ne le journalise
  pas. Le gateway et le dashboard conservent leur réseau interne ; ajouter un bridge
  donnant au gateway une sortie générale n'est pas autorisé par cette direction.
- Le raccord logique/physique est explicite et validé avant admission : tenant,
  autorité et origine de la session ne changent pas silencieusement. Ne pas
  ajouter un champ à un plan fermé V1/2/3 ; une évolution exige compatibilité,
  migration et preuves. La représentation retenue sera relue avec les coutures
  clients après composition du lot I.
- Restreindre aussi l'accès réseau au contrôle quand le déploiement le permet.
  `internal:true`, loopback, un port distinct ou un socket mode 0600 ne sont
  jamais une authentification applicative. L'OS ne distingue pas les programmes
  du même UID. Un UDS optionnel ne sera utilisé que là où ses propriétés sont
  vérifiées ; ne pas supposer qu'un bind de socket traverse Docker Desktop macOS.

## Portée et limites

Le principal visé est le client sans credential pouvant joindre les accès
publiés, indépendamment de son UID. Il ne doit pas retenir les places réservées
au runner par ses uploads publics. Cela ne constitue pas une résistance globale
à un nombre illimité de connexions, à la pénurie de FDs ou à une panne du store.
L'admission TCP/en-têtes/idle du lot L et les coûts d'authentification/couplages
inter-stores du lot K restent ouverts et doivent être traités séparément.

Une erreur de transport après envoi peut laisser un résultat durable inconnu.
Conserver l'identité de l'opération et réconcilier son reçu exact ; ni annulation
ni absence de réponse ne prouvent un rollback ou une absence d'effet.

L'accord de réessai du moniteur, [lot L](GATEWAY_TRANSIENT_FAILURE_PROPOSAL.md),
n'est pas une solution à ce partage de capacité. Ses soixante secondes ne
s'appliquent ni aux claims, ni aux lancements, ni à leur audit obligatoire.

## Répartition et preuves avant intégration

L'auteur H possède le raccord listener/admission, `main.rs`, les configurations
Compose concernées et leurs tests. Root coordonne les coutures de `api.rs`,
`runner.rs` et des clients/brokers après composition I ; aucun fichier partagé
n'est édité simultanément. Les guides local/broker anglais et français sont
mis à jour avec les options réellement implémentées.

Reproduire d'abord le résidu sur la source courante, puis conserver les tests :

- Anonyme/viewer/worker refusés avant consommation du corps, sans slot réservé.
- Seize uploads et soixante-quatre échanges publics retenus ne bloquent pas
  une transition et un claim autorisés, avec leurs vrais reçus durables.
- Capture déclarée hors canal ; aucun fallback ni sélection par données agent.
- Panne d'audit, refus tenant/rôle, corps incomplet/trop grand et erreurs de
  transport sans effet partiel ni nouvelle identité d'opération.
- Contrats de configuration et de reprise compatibles avec les plans conservés.
- Raccord Docker et Podman réels, puis restrictions cloud, qualifiés séparément
  des tests logiciels HTTP/SQLite locaux. Ni mock ni loopback ne ferme ce gate.

Luna exécute les sélections utiles dans la QA isolée. Une revue indépendante
traite les refus, la compatibilité et la séparation des budgets avant commit,
push et fermeture du worktree. Les preuves et limites rejoignent le registre
d'audit ; aucun constat externe ni gate global n'est fermé par cette proposition.
