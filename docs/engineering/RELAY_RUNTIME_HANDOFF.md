# Raccord privé du relais aux workers — proposition du 7 novembre 2026

**Proposition pour revue Root avant implémentation. Aucun canal runtime n'est
livré ou autorisé par ce document.** Le
[client Rust](../setup/relay/README.fr.md) et les
[routes HTTP/SQLite](../setup/workloads/README.fr.md#autoriser-des-messages-dirigés)
existent ; les workers en `network:none` n'ont toujours aucun chemin vers ces
routes. Les 31 tests logiciels du lot client ne constituent pas trois workers
réels. Voir [preuves et limites](evidence/2026-11-07/relay-client/README.md).

## Frontière proposée

Ajouter un canal privé borné entre une tentative worker et le contrôle Rust
existant, plutôt qu'une ouverture du réseau général du worker. Le contrôle
associerait ce canal à l'identité authentifiée du tenant, au worker, à sa
tentative/génération, à son binding workload et à son autorité runtime prouvée.
Le pathname, l'UID seul, une adresse réseau ou un identifiant JSON ne suffisent
pas à établir ce lien.

Le canal ne transporterait que les trois opérations worker existantes :
`POST /api/v1/relay/messages`, `GET /api/v1/relay/messages` avec curseur/limite
fermés et `POST /api/v1/relay/messages/{id}/ack`. Un dispatch Rust fermé
construirait ces chemins à partir des DTO validés. Aucune URL, méthode, source,
tenant ou credential libre fourni dans une frame ne choisirait son autorité.
Ni routes runner, grants/revoke/prune, registre, décision d'exécution ou accès
au moteur ne seraient exposés. Le routeur `/control` actuel ne fournit pas ce
transport worker : sa liste fermée ne doit pas être élargie par un proxy générique.

Le grant métier resterait distinct du canal runtime. Chaque échange conserverait
l'authentification workload et la revalidation serveur des endpoints, du grant,
de sa durée et de sa révocation. L'audit durable obligatoire précéderait la
mutation/livraison. Une panne bloquerait l'échange, sans exécution sur l'hôte,
preuve alternative ou nouvelle permission. Aucun rôle ou service supplémentaire
n'est requis par principe ; toute nécessité constatée serait motivée et soumise
à la revue d'architecture.

## Comparer avant de choisir

| Candidat | Atout | Condition à prouver avant choix |
|---|---|---|
| Socket Unix privé, co-localisé avec le contrôle Rust et le worker dans le noyau Linux concerné | Conserve `network:none`, canal dédié sans destination TCP libre | Répertoire/volume neuf propre à la tentative, ownership et identité de pair/capability vérifiés, refus des substitutions/liens et de l'adoption d'un ancien socket. Un bind de pathname macOS visible dans Docker Desktop ne prouve pas qu'un socket Unix traverse vers la VM Linux. Même question pour une VM Podman. La co-localisation nécessaire ne peut pas être supposée. |
| IPC runtime dédié, par exemple un flux stdin/stdout attaché et cadré par l'adaptateur de confiance | Peut traverser la frontière hôte macOS → VM via l'autorité moteur déjà détenue par le contrôle, sans socket moteur dans le worker | Prouver l'attachement à l'instance exacte et isoler le flux des logs/PTY du fournisseur. Un processus de transport Rust dédié, ses FDs, la fermeture, les erreurs et la backpressure exigent un contrat. Aucun shell hôte ou argv métier libre ; la disponibilité de cette couture n'est pas livrée. |

La première décision est la topologie effectivement praticable sur Linux natif,
Docker Desktop/macOS et Podman, avec des preuves distinctes. Une adresse loopback
de l'hôte n'est pas celle du worker. Monter simplement un socket hôte ou joindre
le réseau de contrôle n'est pas une solution admise. Kubernetes nécessitera son
propre choix qualifié et ne découle d'aucun de ces candidats locaux.

Le choix doit aussi fixer où demeure la preuve workload exclusive : remise
privée au client Rust de la tentative, ou conservation dans le contrôle avec
preuve authentifiée du pair à chaque demande. La seconde option impose de
qualifier le risque de mandataire confus. Dans les deux cas, aucun runner, jeton
SSO humain, credential fournisseur, `.env` complet ou projet original ne rejoint
ce canal. Pour un tronçon HTTPS autorisé, conserver une origine fixe et une
chaîne publique d'AC immuable, validation chaîne/nom active, sans clé privée
d'AC, repli HTTP ni modification du magasin système. Le
[contrat PKI](INFRASTRUCTURE_PKI.md) s'applique à toute opération distante réelle.

## Préconditions de sécurité et de cycle de vie

- Définir une découverte privée et une capability propre à la tentative, son
  epoch, ses bornes temporelles et sa révocation. Refuser remplacement de chemin,
  de socket, de worker, de binding ou de runtime. La terminaison/révocation ferme
  l'admission ; aucune reprise ne réactive implicitement l'ancien canal.
- Conserver la requête et son ID d'opération avant dispatch, avec un mécanisme
  Rust durable à choisir/revoir. Le client actuel n'a pas de journal. Perte de
  réponse, déconnexion, redémarrage moteur ou contrôle indisponible restent
  `unknown` jusqu'à rapprochement de la même opération ; aucun nouvel ID, start,
  grant ou délai n'est créé pour débloquer la reprise.
- Fixer plafonds de frames, files, connexions, temps et contenu retenu par
  tentative/tenant ; conserver les plafonds HTTP et l'audit existants. Pas de
  blocage illimité de l'arrêt broker derrière un flux ou un écrivain. Ni preuve
  brute ni corps métier dupliqué dans les diagnostics d'audit.
- Conserver provenance, dernière réussite et couverture partielle au contrôle.
  Une déconnexion n'est ni zéro activité ni une preuve d'arrêt. Le relais ne
  filtre pas Internet, les outils Web/MCP distants ou les communications TCP
  générales : le chantier réseau P1 conserve ses propres exigences.

## Qualification exigée après revue du choix

Exécuter la recette réelle A1/A2 chez A et B1 chez B sur chaque moteur/topologie
retenu, sans remplacer ses inspections par des fixtures Running. Prouver envoi
A1→A2 autorisé, refus sans grant/inter-tenant/inverse, ACK sans double effet,
expiration/révocation, refus sur panne d'audit et absence de fuite de preuve.
Mesurer fermeture et visibilité des états périmés dans la vue autorisée.

Exercer perte après commit send et ACK, replays divergents, frames tronquées ou
trop grosses, file saturée, substitution du canal et mauvaises identités, mort
du worker/transport/broker, redémarrage runtime et reprise froide avec les mêmes
identités. Inspecter réseau `none`, montages, FDs, credentials et absence d'accès
au contrôle/moteur/autres tenants. Les critères d'arrêt au plafond et les preuves
de chaque campagne devront être arrêtés avant son exécution.

**Décision encore requise :** choisir et faire relire la couture locale et la
garde de preuve avant toute implémentation ou essai runtime. Les guides EN/FR,
procédures de rotation/arrêt/reprise et preuves réelles accompagneront ce lot.
Ce document ne ferme aucun gate et ne déclenche aucune opération moteur,
réseau, cloud ou PKI.
