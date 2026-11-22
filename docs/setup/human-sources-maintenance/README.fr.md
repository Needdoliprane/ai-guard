# Adopter les sources humaines existantes

[English](README.md) · [Index d'installation](../README.fr.md) · [Accès local nominatif](../identity/README.fr.md) · [SSO](../sso/README.fr.md)

Ce parcours opérateur adopte sur place une union locale existante : chaque
Store client doit déjà être au format13 et le ledger d'authentification
facultatif en Auth4. Il prépare les fichiers privés, appelle le coordinateur
existant13→14/4→5 puis publie la configuration d'activation du gateway.
La [validation logicielle datée](../../engineering/evidence/2026-11-13/human-sources-operator/README.fr.md)
couvre ce parcours et son profil HTTP adopté sur une source Linux ARM64 figée.
Les autres profils de déploiement et la qualification production restent ouverts.

Préparer les identités nominatives et leurs appartenances explicites **avant**
l'adoption avec le [guide de maintenance des identités](../identity/README.fr.md).
Conserver l'historique réel des clients, les fichiers de credentials, l'identité
d'installation et les sauvegardes de migration. Utiliser les migrations legacy
explicites existantes pour atteindre Store13 et, si sélectionné,
[Auth4 avec la politique originale et le plan fini exact des tenants](../sso/README.fr.md).
Cette commande ne crée ni personne ni nouveau credential. Le registre conserve
les références aux fichiers privés runner/viewer existants, avec des secrets
distincts de 32–512 octets UTF-8, sans caractère d’espacement après trim. Aucun secret n'est passé
en argument ni affiché par ce parcours.

Arrêter d'abord **tous** les consommateurs de ces bases.
`--confirm-control-stopped` est une attestation opérateur ; elle ne bloque pas
un ancien processus. La préparation ouvre les vrais wrappers SQLite legacy et
peut modifier l'état SQLite/WAL. Ce n'est pas un diagnostic en lecture seule.
Conserver sources/configurations dans leurs répertoires privés existants ; les
nouveaux parents de préparation, journal et sauvegardes sont distincts des
parents sources. Ne pas déplacer ou remplacer une préparation ou une entrée liée.

Sans OIDC, choisir une destination absolue neuve :

```sh
agentsandbox human-sources prepare \
  --tenant-registry /absolute/control/tenants.json \
  --installation-id 11111111-1111-4111-8111-111111111111 \
  --control-config-revision 7 --max-events 100000 \
  --output-dir /absolute/control/human-preparation \
  --confirm-control-stopped
```

L'UUID ci-dessus est un exemple : employer l'identité d'installation réellement
conservée. Avec OIDC, ajouter ensemble `--oidc-config /absolute/control/oidc.json`
et `--oidc-mapping-revision 9`. La configuration privée OIDC est version2,
conserve un bundle public d'AC explicite et désigne le plan de capacité Auth4
exact des tenants configurés. La révision de mapping est un entier positif
distinct de la révision chaîne du mapping fournisseur. La projection publique
lie les politiques effectives ; credentials et identités physiques privées
restent dans le recovery/seals privés. Les routes HTTP OIDC restent fermées dans
ce profil owned, même si son vrai ledger Auth5 fait partie de l'union.

Le premier document durable, `recovery.json`, fixe les UUID d'opération, sources,
closure et credentials techniques, la génération1, les bindings d'octets des
entrées et les attentes physiques. Les publications suivantes en dérivent.
Catalogue privé complet : `recovery.json`, `technical-credentials.json`,
`maintenance-plan.json`, `preparation-complete.json`, `activation.json`,
`activation-complete.json`, plus `journal/` et `backups/`. L'enveloppe contient
des credentials privés ; ne pas publier ces fichiers ni les joindre à une issue.

```sh
agentsandbox human-sources check \
  --preparation-dir /absolute/control/human-preparation
agentsandbox human-sources apply \
  --preparation-dir /absolute/control/human-preparation \
  --confirm-control-stopped
agentsandbox gateway \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --jev-backend rust-rules
```

Le flag d'activation est le trajet de démarrage normal. Ne pas le combiner avec
les options directes de sources, registre, credentials, révisions, capacité,
initialisation ou OIDC. Bind HTTP, TLS serveur et réglages de processus/décision
gardent leur configuration habituelle. Le gateway reconstruit indépendamment
l'union attendue puis rejoue acquisition owner, SQLite, planchers durables, CAS
d'incarnation et rétention des sources à **chaque** démarrage. Un manifest ou un
seal de publication seul n'autorise aucune source et ne prouve aucune incarnation.

Après interruption, conserver sur place toutes les sources et pièces :

```sh
agentsandbox human-sources resume-prepare \
  --preparation-dir /absolute/control/human-preparation \
  --confirm-control-stopped
agentsandbox human-sources resume \
  --preparation-dir /absolute/control/human-preparation \
  --confirm-control-stopped
```

`resume-prepare` reprend les publications déterministes. Les fichiers suivants
absents sont créés exclusivement ; les fichiers complets identiques sont
revalidés et synchronisés. Un dossier sans recovery valide, une entrée changée
(même avant la première enveloppe), ou un document tronqué/divergent constitue
un conflit. Aucun overwrite, réparation, suppression ou nouvel UUID. Réconcilier
explicitement les preuves préservées avant de poursuivre. `resume` exige une
vraie intention du coordinateur ou une activation complète publiée ; il ne crée
pas d'opération neuve. `apply` peut démarrer l'opération persistée. Les deux
transmettent au coordinateur les mêmes plan, backups, journal et révisions.
Les commits partiels sont conservés.

Le coordinateur revérifie attentes physiques/logiques/configuration/format et
intégrité SQLite sous son union retenue et ses writers. Le backup et le préfixe
d'historique sont observés **à apply**. Prepare ne gèle pas tous les changements
sur le même inode ; une évolution valide conservant les attentes enregistrées
peut être examinée par le coordinateur. Aucun head préparé ni fencing legacy
promis. Les admissions du startup temporaire sont fermées et drainées avant la
publication d'activation. Si activation et seal étaient déjà publiés à
l'identique, la reprise renvoie `already_published_files` sans nouvelle
invocation coordinateur/startup. Cet ACK ne revérifie pas SQLite.

`check` valide seulement fichiers privés et métadonnées. Il ne lit aucun contenu
SQLite main/WAL/SHM ou backup, n'acquiert aucun owner, n'écrit et ne synchronise
rien. Résultats : `prepared_files_valid`, `activation_files_valid`, `incomplete`
ou `conflict`. Ce sont des statuts de fichiers, jamais Ready, intégrité des
bases, authentification réussie ou absence d'activité. Une migration/activation
partielle exige une reprise explicite. Les publications sont ordonnées, seal en
dernier ; aucune atomicité multi-fichiers/multi-bases ni qualification de coupure
électrique n'est annoncée.

Après démarrage, employer un credential **humain** privé avec les
[commandes identity/resources](../identity/README.fr.md) ; les tokens runner/viewer
restent techniques. Le profil HTTP owned disponible est décrit dans
[sources owned](../identity/OWNED_SOURCES.fr.md).
Lorsque d'autres sources tenants sont sautées, `contexts_complete=false`
signifie que leurs appartenances restent inconnues ; une union à un seul tenant
peut annoncer des contextes complets. Les lectures identity/resources et leurs
reçus durables ne qualifient ni les autres handlers, ni brokers distants, cloud,
reprise après coupure physique ou préparation à la production.

## Préparer un tenant pour le registre des politiques egress

La commande de maintenance SQLite fait passer un Store14 d'une installation
owned adoptée au format 15. Arrêter le gateway et tous les consommateurs.
Conserver l'activation précédente et choisir un répertoire de sortie absolu,
neuf et extérieur aux répertoires sources. Générer un UUID d'opération et le
conserver avec la commande.

```sh
agentsandbox human-sources upgrade-egress \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --tenant tenant-a --operation-id 22222222-2222-4222-8222-222222222222 \
  --output-dir /absolute/control/egress-tenant-a --confirm-control-stopped
```

Remplacer le tenant, l'UUID et les chemins par ceux de l'installation. Si la
configuration courante utilise des références explicites, fournir à la place
de l'activation `--human-sources-manifest`, `--human-credentials-envelope`,
`--tenants-file` et `--human-control-config-revision`. Reprendre aussi
`--max-events` si personnalisé, ainsi que `--oidc-config` et
`--human-oidc-mapping-revision` si sélectionnés.

La commande sauvegarde une copie vérifiée dans `backup/source.sqlite` et
confirme la genèse egress durable, qui refuse toute destination. Elle publie
ensuite `closure/manifest.json`. Au résultat `store15_closure_published`,
sélectionner ce manifeste et `technical-credentials.json` du même répertoire
de sortie dans la configuration explicite `owned_sources` du gateway. Garder
les autres entrées de configuration et leurs révisions. L'ancien manifeste
est devenu périmé pour la source migrée. Pour préparer un autre tenant,
utiliser la dernière clôture publiée. L'application au runtime relève de sa
propre qualification.

Après une interruption, conserver tous les fichiers. Relancer la même commande
avec `--resume`, la même activation ou le même manifeste antérieur, le même UUID
et le même répertoire, une fois l'arrêt des contrôles vérifié. Une clôture
existante doit correspondre à la genèse confirmée. Si le répertoire de sortie
final n'a jamais été publié, préserver son répertoire de préparation voisin et
repartir dans une nouvelle destination après vérification hors ligne que la
source est toujours au format 14. Une sauvegarde sans reçu enregistré demande
une réconciliation avant reprise. Conserver la copie et le journal pour le
diagnostic. L'état de disponibilité du gateway sera établi à son démarrage.

## Périmètre de validation

Les [preuves datées du parcours opérateur et du startup owned](../../engineering/evidence/2026-11-13/human-sources-operator/README.fr.md)
consignent **107 fonctions nommées uniques PASS sur la même source figée**, en
**deux campagnes distinctes : 34 puis 73**, sans fonction en échec ou ignorée ni
omission acceptée. Il ne s'agit pas d'un run de 107 tests. Le check toutes cibles
de la première campagne passe ; la seconde reprend explicitement ce résultat
sur la source exacte sans répéter le check. Chaque campagne compile les
artefacts de test sélectionnés et vérifie l'aide du CLI normal. Les preuves
consignent enregistrements réels, résultats, revue indépendante, conservation
des sources et fermeture des ressources.

Le [registre initial d'implémentation W0](../../engineering/evidence/2026-11-13/human-owned-startup/README.fr.md)
conserve sa source historique et son `NOT_RUN` au moment de l'implémentation ;
son ajout renvoie à la validation ultérieure. Les
[preuves antérieures de validation des sources](../../engineering/evidence/2026-11-13/human-source-validation/README.fr.md)
gardent aussi leurs sources et résultats limités. Ces historiques ne sont ni
réécrits ni transférés à un autre checkout.

La validation couvre le logiciel Linux ARM64 isolé, avec de vraies fixtures
SQLite privées et le comportement HTTP/CLI local. Elle ne qualifie ni coupure
électrique physique, macOS natif, workers métier ou leurs adaptateurs
Docker/Podman/Kubernetes, IdP ou fournisseur réel, frontend/navigateur, suite
complète, Clippy strict, release ou 100 workers. Le profil owned adopte
délibérément les seuls GET identité et ressources ; OIDC sélectionné reste
retenu avec ses handlers connus fermés. Approbation/lancement humain, contrôle
egress et provisionnement moderne d'identités restent hors de ce profil.
Aucun gate de production ni constat d'audit n'est clos.
