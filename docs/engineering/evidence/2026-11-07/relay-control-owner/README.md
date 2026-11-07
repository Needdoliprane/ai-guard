# Propriétaire contrôle du relais — gel 66, 7 novembre 2026

Base : `410d4d24bd75c02dd6262864412ec637a0d45bed`, worktree
`codex/relay-trusted-owner-20261107`. Les six sources Rust et deux entrées Cargo
du [manifeste du composant](source.sha256) correspondent au gel `relay-owner66`
vérifié par Root et testé dans QA AL. Les deux guides ont été finalisés ensuite,
sans modifier ces huit entrées. Le manifeste inclut également leurs empreintes.
Ces résultats ne se transfèrent pas à un autre checkout par partage de base.

La [direction approuvée](../../../RELAY_RUNTIME_HANDOFF.md) autorise code et
tests logiciels locaux, pas une campagne moteur du nouveau transport. Les
guides [EN](../../../../setup/relay/CONTROL.md)/[FR](../../../../setup/relay/CONTROL.fr.md)
décrivent format, réservations et intégration obligatoire. Le composant reste
privé : aucun constructeur d'admission production et refus systématique de
`current_admission` hors tests. Il ne rend pas le relais utilisable depuis un
worker réel.

## Résultats exécutés et périmètre

Luna a exécuté trois sélections sur les mêmes sources figées, avec le target
dédié `/scratch/target-relay-owner66-al`. Les douze nouveaux parents et les
dix-neuf canaris existants sont distincts : **31 fonctions uniques PASS**.
Les filtrés ne comptent pas comme tests passés ni comme tests supplémentaires.

| Sélection observée sur 66 | Résultat du journal | SHA-256 du journal parent |
|---|---|---|
| `relay::channel::control::tests::` | PASS 12/12, 0 ignoré, 629 filtrés, 0,56 s ; HOST_RC=0 | `4769d95023335f41869ae20b46e26dd359b982f49246d22a7b2a7e4d42a1319a` |
| `worker_relay_api`, `client_cases::` | PASS 13/13, 0 ignoré, 27 filtrés, 8,06 s ; HOST_RC=0 | `9209ce2ca03b2a1de0c6aa5d337f1e5bfa97fa05c4d5565131834a97b51f2ccd` |
| `worker_relay_api`, `client_profile_cases::` | PASS 6/6, 0 ignoré, 34 filtrés, 3,48 s ; HOST_RC=0 | `4f4eeb961252da34d319a86cd5e7233b64c817306566c2f9fa8146d89e569e59` |
| Compilation nécessaire à ces sélections | PASS ; première compilation lib/test 1 min 55 s, puis cible client 54,98 s | Journaux ci-dessus ; cela ne remplace pas toutes cibles |
| Compilation toutes cibles de66, QA AO | PASS ; RC0,88,93s | `5b7afad56b4fb66a003640c1901d3cb0bbf30476709f360f7cc8749c131622f0` |
| Clippy strict de 66 | NOT_RUN | Aucun verdict global G12 déduit |
| Rustfmt direct et `git diff --check` du composant | PASS local trusted, sans exécution du projet | Vérifiés après source 66 |
| Docker/Podman/Kubernetes, fournisseurs, recette trois workers | NOT_RUN | Hors campagne logicielle du composant |

Le registre d'exécution communiqué par l'opérateur Luna confirme les argv
ci-dessous, depuis `/scratch/relay-owner66`. Ces argv **ne sont pas imprimés
dans les journaux parent** ; leur provenance est le registre de l'opérateur,
sans relance des tests. Le binaire Cargo absolu est
`/usr/local/rustup/toolchains/1.99.1-aarch64-unknown-linux-gnu/bin/cargo` :

```text
cargo test --lib --locked --offline relay::channel::control::tests:: -- --test-threads=1 --nocapture
cargo test --test worker_relay_api --locked --offline client_cases:: -- --test-threads=1 --nocapture
cargo test --test worker_relay_api --locked --offline client_profile_cases:: -- --test-threads=1 --nocapture
```

Les nouveaux parents emploient de vrais sockets Unix, TCP/Axum loopback et
fichiers privés synchronisés. Les reçus du pair HTTP de ces douze parents sont
des fixtures déclarées : ils ne prouvent pas l'authentification du gateway,
un audit SQLite, une causalité humaine ou un runtime Running. Les treize canaris
client et six canaris de profils réexercent séparément le vrai HTTP/SQLite,
les bindings workload, les frontières de rôles/endpoints, les replays send/ACK,
les pertes de réponse, les DTO fermés et les profils fixe racine/`ControlIngress`.
Ils ne changent pas la provenance synthétique des observations runtime de
leurs fixtures, décrite dans les preuves du [client](../relay-client/README.md).

Le check toutes cibles ultérieur utilise les mêmes six sources Rust et Cargo,
dans QA AO, avec une cible de compilation distincte. Les premiers essais AN
échouaient avant compilation sur cache public incomplet ; ils ne constituent
pas des échecs produit. Le cache public copié et son complément
`linux-raw-sys0.12.1` vérifié contre Cargo.lock permettent ensuite la validation
hors ligne. Le journal parent `owner66-check-ao.log` ci-dessus fait autorité ;
le check n'exécute pas de tests supplémentaires et ne vaut pas Clippy strict.

Le test d'annulation suspend un vrai segment bloquant avant son opération,
avec attente bornée à cinq secondes, puis annule `serve`. Il vérifie handle,
slots tenant/global distincts, mémoire logique, `RoleLease` et flock retenus,
refus d'une seconde admission puis jonction explicite après libération. Il ne
prouve pas une panne disque ou une fin autonome de helper/worker.

Le test d'inbox dense utilise 32 corps de 60 000 octets et un socket Unix dont
`SO_SNDBUF` est réglé puis relu à au plus 65 536 octets. Après le seul préfixe
d'une réponse de plus de 512 Kio, la sortie reste bloquée et les mêmes permits
sont encore détenus. Après annulation et jonction, ils sont rendus. La réserve
3 Mio et les plafonds 32/128 Mio sont des **unités logiques d'admission** : DTO,
valeurs JSON et frame encodée peuvent coexister au-delà de 3 Mio. Somme des
buffers vivants, RSS et capacité runtime restent NOT_RUN.

## Revue de développement et échec conservé

La revue indépendante a relevé deux P2 sur la candidate en développement :
réservation rendue avant sortie et absence de contrôle de secrets sur le lock
constant/le digest de dispatch. Les correctifs conservent l'Arc jusqu'à la fin
réelle de sortie, gardent ses clones dans les IO en attente et centralisent le
contrôle de tous les records avant écriture. Les noms d'opérations sont aussi
contrôlés avant création et à la réouverture.

Le canari de métadonnées vérifie preuve ASCII égale au lock, preuve hex64 égale
au digest de la requête et secret connu égal au nom `op-UUID`, sans recherche
de préimage. Ces formats sont acceptés à la frontière du composant ; le CLI
standard émet `aigw1_<hex>`. Ce sont des constats de revue de développement,
pas un exploit démontré de ce profil standard. Ils ne modifient pas les treize
vulnérabilités historiques du [rapport primaire run-1](../../../AUDIT_FOLLOWUP.md#audit-final-run-1).
La relecture source bornée du gel 64 a accepté les deux corrections ; les
régressions passent dans la sélection 66 ci-dessus. Aucun RED comportemental
exécuté pour ces deux constats n'est revendiqué.

La première tentative QA du gel 64 échoue à la compilation Rust E0716 : la
frame de réponse temporaire est empruntée par la future de sortie dans le
`select`. Aucun parent ne s'est exécuté. Le journal
`relay-owner64-tests-al.log`, HOST_RC=101, porte SHA-256
`103f850a555cfdcd7242ffb44ff24716399ed6fc2ae795cdb549c9e7a1ec3004`.
Ce n'est pas un RED métier. Le seul correctif Rust de 64 à 66 lie cette frame
à une variable avant les futures ; il n'ajoute aucun clone et conserve la
réservation. La source 66 n'est pas modifiée après les résultats ci-dessus.

## Environnement et pièces retenues

QA AL utilise Linux ARM64 et Rust/Cargo 1.99.1 selon le registre d'opérateur,
avec l'image épinglée
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
L'inspection retenue confirme UID/GID 1000, réseau `none`, aucun port publié,
racine readonly, capacités supprimées, non privilégié et
`no-new-privileges:true`, 4 CPU, 8 Gio RAM sans marge de swap, 256 PIDs,
scratch tmpfs 4 Gio/65 536 inodes, fichiers de 512 Mio, 1024 FDs et core 0.
Inputs et caches publics sont montés readonly ; le target est propre à 66.
Ces paramètres de QA ne qualifient pas le confinement d'un worker du produit.

Les journaux parent lus et rehashés restent sous
`/private/tmp/ai-guard-cold-drain-logs-20261106.nXx9XJ/` :
`relay-owner66-tests-al.log`, `relay-owner66-client-al.log`,
`relay-owner66-profiles-al.log`, l'échec `relay-owner64-tests-al.log` et le
manifeste intégral `relay-owner66-source.sha256`.
L'inspection initiale `qa-al-inspect.json` a SHA-256
`ad1324793b83cf28048dafadf08ba149215369f0e7e3edbd1064a32a3621f39d`.
L'inspection finale `qa-al-final-inspect.json` a SHA-256
`100f6f97479f910e88bce17ff130dc12ff9a5648182185a19283d9081ab6c5db` :
état `exited`, code 137, `OOMKilled:false`. Cet état du conteneur QA est distinct
des trois HOST_RC=0 ; aucune fin propre du produit ni mesure RSS n'en est déduite.
Ces chemins temporaires privés ne sont pas un canal public ni une procédure
de sauvegarde. Aucun exécutable, base, credential réel ou payload runtime
n'est ajouté à ce dossier de preuve.

## Écarts conservés

La factory production reste fermée jusqu'au raccord par Root de l'admission et
du runtime exact. Registry service, budgets partagés effectifs, records durables
de canal/epoch, permis relay-exec distinct, observation de l'incarnation,
réservation d'audit obligatoire, child/stderr/fermeture runtime et révocation
restent à intégrer et qualifier. Le format conserve prudemment tout dispatch
sans reçu validé, même après refus certain : classification des refus et
procédure opérateur de rapprochement restent obligatoires. Ancrage signé,
durabilité physique après crash, sauvegarde/restauration et coût réel des caps
ne sont pas qualifiés.

Les 31 fonctions ne prouvent ni transport runtime complet, trois vrais workers,
fournisseur, TLS entreprise/mTLS, multi-hôtes ou capacité 100. STOP/enforcer,
observations terminales, libération du quota et usage ne sont pas déduits d'EOF,
d'un reçu ou d'une fermeture locale. Aucun gate clos, aucune release qualifiée.
