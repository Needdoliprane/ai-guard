# Adopter les sources humaines existantes

Ce parcours opérateur adopte sur place une union locale existante : chaque
Store client doit déjà être au format13 et le ledger d'authentification
facultatif en Auth4. Il prépare les fichiers privés, appelle le coordinateur
existant13→14/4→5 puis publie la configuration d'activation du gateway.
L'implémentation et ses tests sont candidats ; les nouveaux tests bootstrap
restent **NOT_RUN** jusqu'à un rapport daté indiquant sources et commandes
réellement exécutées. Ce guide ne ferme aucun gate de production.

Préparer les identités nominatives et leurs appartenances explicites **avant**
l'adoption avec le [guide de maintenance des identités](../identity/README.fr.md).
Conserver l'historique réel des clients, les fichiers de credentials, l'identité
d'installation et les sauvegardes de migration. Utiliser les migrations legacy
explicites existantes pour atteindre Store13 et, si sélectionné,
[Auth4 avec la politique originale et le plan fini exact des tenants](../sso/README.fr.md).
Cette commande ne crée ni personne ni nouveau credential. Le registre conserve
les références aux fichiers privés runner/viewer existants, avec des secrets
distincts de32–512 caractères sans espace après trim. Aucun secret n'est passé
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
[sources owned](../identity/OWNED_SOURCES.fr.md). Il conserve
`contexts_complete=false` ; lectures identity/resources et reçus durables ne
qualifient ni les autres handlers, ni brokers distants, cloud, reprise après
coupure physique ou préparation à la production.
