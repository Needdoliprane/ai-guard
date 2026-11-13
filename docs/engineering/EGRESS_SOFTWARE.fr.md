# Contrats logiciels des sorties réseau

[English](EGRESS_SOFTWARE.md) · [Direction autorisée et exigences complètes](EGRESS_CONTROL.md) · [Preuves de validation](evidence/2026-11-13/egress-core-protocol/README.md)

La bibliothèque Rust contient maintenant les contrats `egress` de politique, de parsing CONNECT et d’état des flux. Ce sont des primitives logicielles utilisables et testées. Elles n’exposent ni listener proxy, route gateway ou nouveau mode runtime. Le réseau des workers reste `none` ou `bridge` explicitement demandé ; les sorties bridge restent non médiées. Aucun nouveau réglage n’active le filtrage des destinations.

## Comportement implémenté

- `Authority` accepte un nom DNS ASCII exact et un port explicite non nul. La casse est normalisée ; URL, userinfo, jokers, IP littérales et formes numériques ambiguës sont refusés. Unicode et alias avec point final restent hors du profil fermé. Une autorité wire est bornée à 512 octets, le nom canonique à 253 et chaque label à 63. Les erreurs ne recopient pas l’entrée.
- `EgressPolicyV1` est un snapshot immuable et révisionné. L’évaluation effective exige deux politiques actives, tenant et session, autorisant exactement nom/port/transport. Une politique vide refuse. Le digest canonique SHA256 lie révision, intervalle, destinations et limites. Le plafond est 128 destinations ; les limites de politique peuvent être réduites dans cette enveloppe.
- `ValidatedResolution` valide au plus 16 candidats IPv4/IPv6 avant copie, refuse l’ensemble dès qu’une adresse est spéciale/locale, puis trie/déduplique les candidats acceptés. Il ne résout aucun nom et ne prouve pas l’identité du résolveur. Le classifieur conservateur ne reconnaît pas un service de contrôle ayant une adresse publique ordinaire ; les exclusions contextuelles restent nécessaires.
- Le frontend emprunté à `httparse` accepte un profil HTTP/1.1 CONNECT fermé : un seul Host correspondant, 16 Kio d’en-têtes et 32 champs au plus. CL/TE, headers d’authentification, demandes ambiguës et données de tunnel anticipées sont refusés. La réponse de succès exige une transition courante d’acceptation broker ; cet état local n’authentifie pas le broker.
- Les payloads fermés utilisent du base64 canonique borné et un préflight à forme fixe avant appropriation variable du contenu. Data est borné à 16 Kio décodés, sa trame à 24 Kio ; les autres trames à 2 Kio. Aucun second framing à préfixe de longueur ni élargissement du relais V1/V2 n’est ajouté.
- Le registre pur possède 16 slots, quatre ouvertures en attente, des IDs monotones, quatre ouvertures/s avec rafale 8, des délais et du crédit séparé par sens. Crédit et bytes en queue sont bornés à 256 Kio par sens. HalfClose libère le crédit inutilisé et conserve les réservations des bytes déjà admis jusqu’au drain/abandon. Reset abandonne les réservations. Ce sont des états numériques, pas des opérations socket observées.

La capacité V1 actuelle exige ses limites de flux fixes exactes. Une négociation à la baisse comme à la hausse est refusée ; les profils runtime configurables ne sont pas livrés par cette tranche. Limites de politique et de négociation des flux restent distinctes.

## Autorité et raccords restants

`PolicyAllowed`, `ValidatedResolution` et le message `Accepted` sont descriptifs. Aucun ne crée un permis durable de connexion, ne choisit un tenant ou ne prouve le binding runtime. Le futur propriétaire de confiance doit authentifier le worker/époque exact, committer l’audit obligatoire avant connect, épingler l’adresse numérique validée et posséder/fermer les vrais sockets. Il doit aussi réserver les buffers réels de framing/JSON/IO, préserver STOP et appliquer révocation/expiration. Les comptes logiques ne mesurent pas la RAM du processus.

Dans le futur chemin raccordé, CONNECT n’autorise qu’un transport TCP opaque. Il ne prouve ni HTTPS effectif, contrôle des certificats ou absence de tunnel. La garde des clés fournisseur reste une frontière séparée ; aucune protection de clé contre le worker n’est ajoutée ici. Le frontend ne détient aucun credential ai-guard.

Administration Store/API, grants et réserves d’audit, émetteur de permis fiable, négociation du canal commun, listener/bootstrap/runtime et dashboard restent à raccorder. Aucune activation Internet de worker, installation de capteur, MITM/PKI ou action cluster n’a été effectuée. Visibilité des tentatives directes, Web/MCP distants, quatre modes fournisseur et recettes Docker/Podman/Kubernetes restent obligatoires et ouverts.

## Validation et reproduction

La composition figée passe 34 parents purs sélectionnés : 14 politique, 6 frontend, 11 protocole et 3 parents du mapper d’identité existant rejoués dans cet arbre. Check toutes cibles et formatage passent. Les deux défauts de revue ont d’abord été reproduits par deux échecs réels, puis corrigés et relus indépendamment. Les [preuves](evidence/2026-11-13/egress-core-protocol/README.md) identifient les sources et leur périmètre.

Pour les 31 parents egress de cette source, la sélection Cargo équivalente s’exécute dans la QA isolée exigée par le dépôt :

```console
cargo test --frozen --offline --lib egress:: -- --test-threads=1
```

Ce n’est pas un test runtime, de débit ou de 100 workers. Aucun gate de production n’est clos par ce résultat logiciel.
