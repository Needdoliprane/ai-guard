# Invocation de confiance — livraison core, politique2

29 octobre2026. Checkout sans HEAD, travail existant préservé. Propriétaire : trusted_runtime_core. Aucun moteur réel, cloud, PKI ou NAS utilisé par ce sous-agent. Ce rapport remplace le bilan intermédiaire de politique1 : le cwd '/' et l'acceptation automatique gid80 n'appartiennent plus au code livré.

## Changements livrés au dernier gel

Périmètre exclusif modifié : src/runtime.rs, src/runtime/managed.rs, nouveau src/runtime/invocation.rs ; tests/runtime_managed.rs, runtime_retention.rs, runtime_races.rs, runtime_terminal.rs ; fixtures managed.rs/create_conflict.rs. Aucun contrat/document du dépôt ni caller modifié par ce sous-agent. L'orchestrateur possède ces documents et la QA réelle ; trusted_runtime_caller possède runner/operations et leurs fixtures.

- Fabrique statique Docker/Podman, aucun PATH ambiant consulté pour choisir le CLI. Une entrée existante refusée bloque, sans fallback. Installations personnalisées par chemin absolu explicite. La fabrique reconnaît Docker.app, Homebrew, Linux et /opt/podman/bin/podman de l'installeur officiel macOS, sous les mêmes validations.
- Runtime conserve un FD du répertoire final dans un Arc privé. L'ouverture parcourt le chemin canonique depuis FD '/' par openat(O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC|O_NONBLOCK) pour chaque composant. Un ancêtre devenu symlink après canonicalisation ne redirige donc pas le pinning initial.
- Le leaf directory et le fichier doivent appartenir à root ou au contrôle, sans022. Aucune exception gid80. macOS : toute ACL étendue non vide est refusée sur les deux FDs ; les erreurs d'inspection refusent, seul NULL/ENOENT d'absence FILESEC_ACL et l'ACL vide attestée sont admis. Linux : les modes excluent le write effectif des POSIX ACL locales via leur mask ; pas de qualification NFSv4/SMB/FUSE.
- Avant chaque commande : vérification identité/mode/ACL du FD répertoire, openat du basename sans suivre de lien, fichier régulier/exécutable, identité et hash. Lecture SHA256512MiB maximum ; taille refusée avant lecture ; au plus initial_len+1 octets par blocs64KiB ; EOF, croissance ou changement des métadonnées refusés. O_NONBLOCK ferme le blocage FIFO avant fstat.
- Exécution exclusivement './basename' avec pre_exec fchdir du FD retenu, sans shell. Le Command retient lui-même le FD même si le Runtime est détruit. Le cwd réel est le répertoire CLI ancré hors projet ; sa provenance canonique est persistée. Une substitution du parent après construction du Command exécute encore le binaire approuvé, jamais son successeur. Le nom de provenance peut changer après rename ; l'identité de répertoire ancrée reste la preuve.
- env_clear, HOME issu de getpwuid_r, configurations standards explicites du compte, LANG/LC_ALL fixes. PATH helpers limité à /usr/bin:/bin:/usr/sbin:/sbin, répertoires vérifiés root/sans022 et ACL selon plateforme. Aucun Homebrew,/usr/local ou /opt/podman dans ce PATH. Le CLI principal peut être installé ailleurs grâce à son FD.
- cli_env ne permet que AGENTSANDBOX_CONFIG_DIR explicite absolu. Sélecteurs moteurs/loaders et variables arbitraires ambiants ne sont pas copiés.
- Les fixtures sont natives Rust, approuvées par entrée absolue et métadonnées privées siblings executable.with_extension("fixture"). Aucun bypass applicatif de test par PATH/env.
- Erreurs typées RuntimeCommandFailure, captures opaques et redactions privées préservées.

## API et migration

API publique stable : Runtime::new(kind)->Result<Self>, with_program(kind,&Path)->Result<Self>, from_invocation(kind,InvocationBindingV1)->Result<Self>, invocation()->&InvocationBindingV1, command()->Result<tokio::process::Command>. Runtime Clone, plus Copy.

Le nom du format public InvocationBindingV1 reste conservé pour compatibilité de source ; sa politique d'exécution est policy_version=2. Il contient kind, approved_entry, canonical_program, file_identity(device,inode,owner,group,mode,length,sha256), environment, cwd et executable_directory:Option<ExecutableDirectoryIdentityV1> avec serde(default,skip_none). Cette identité répertoire contient device,inode,owner,group,mode. Les structures refusent les champs inconnus. Debug du binding/anchor reste opaque aux chemins/environnement.

Une ancienne policy1 Some sans executable_directory se désérialise encore (status/lecture privée restent possibles) puis échoue explicitement avant tout accès moteur avec message de migration ; aucune conversion implicite cwd '/'→leaf. Une ancienne authority None exige aussi migration explicite. RuntimeAuthority::fingerprint moteur demeure inchangé, excluant invocation ; invocation_fingerprint est séparé. Les handles existants gardent leur autorité.

ManagedRuntime::{connect_docker_with_runtime,bootstrap_podman_with_runtime,reopen_with_runtime} réutilisent le même Runtime approuvé. adopt_invocation(binding,runtime) accepte le cas explicitement autorisé None ou Some ancienne/modifiée, vérifie le même kind/moteur Docker ou roots+marker Podman uniquement par info/inspect, et remplace la preuve d'invocation. Aucun bootstrap/create/start/kill/remove, aucun claim réarmé. Caller responsable de l'autorisation opérateur, audit durable et checkpoint avant tout effet. La sélection Podman provient de l'environnement approuvé ; les anciennes valeurs selection_env restent sérialisées pour ne pas changer les fingerprints moteur, mais ne sont jamais rejouées.

## Preuves RED conservées

- /tmp/ai-guard-core-red.log : avant correction, le docker factice du projet est exécuté via PATH='.' ; assertion de refus échoue.
- /tmp/ai-guard-core-fifo-red.log : FIFO sans writer bloque avant CLI timeout ; subprocess privé tué par son watchdog2s, échec attendu.
- /tmp/ai-guard-core-podman-red.log : emplacement officiel /opt/podman refusé avant ajout de cette entrée fixe.
- /tmp/ai-guard-core-anchor-red.log : trois échecs attendus avant ancrage/ACL/cap. Parent remplacé exécute réellement SUBSTITUTED ; ACL étendue ignorée ; sparse512MiB+1 lu au lieu du refus par taille.
- /tmp/ai-guard-core-pinning-red.log : open(fullpath) suivait un ancêtre remplacé en symlink après canonicalisation ; échec attendu de la régression avant parcours openat sans liens.

FAIL intermédiaire transparent : /tmp/ai-guard-core-green4.log, missing_oom a partagé la deadline50ms du vrai cas timeout ; sous charge le select déclarait timeout malgré l'exit immédiat de fixture. Les cas non-timeout disposent désormais de2s ; seul timeout garde50ms. Correctif de fixture, pas changement de deadline produit.

## Résultats constatés

Toolchain hôte : rustc1.99.1 (58cf8f9c3 2026-10-13), cargo1.99.1 (5f94df478 2026-08-27), Darwin27.2.0 ARM64.

- PASS final après parcours openat : `CARGO_INCREMENTAL=0 cargo test --locked --lib runtime::`,5/5,0ignoré, /tmp/ai-guard-core-anchor-unit-final.log. Inclut croissance réelle du fichier pendant lecture (exactement initial_len+1 octets lus), hash exact/troncature, symlink d'ancêtre, emplacement Podman et capture typée opaque.
- PASS politique2 avant seul renfort du parcours openat : `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_managed --test runtime_retention --test runtime_races --test runtime_terminal`,19tests (14+1+3+1),0ignoré ; /tmp/ai-guard-core-anchor-integrations.log. Managed couvre substitution du parent avec Command vivant après drop Runtime, ACL fichier/directory, cap sparse, ancien binding Some policy1 décodable/adoptable readonly, env/cwd/helperPATH, inode/contenu et scénarios lifecycle.
- PASS final clippy après parcours openat : `cargo clippy --locked --lib --test runtime_managed --test runtime_retention --test runtime_races --test runtime_terminal -- -D warnings`, /tmp/ai-guard-core-anchor-clippy-final.log.
- Format final fichiers possédés vérifié par rustfmt --edition2024 --check ; log /tmp/ai-guard-core-anchor-fmt-final.log.
- PASS dernière relance managed après parcours openat : `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_managed`,14/14,0ignoré,19.63s sous charge ; /tmp/ai-guard-core-anchor-managed-final.log.

Les autres logs green2/green5/green6 et clippy intermédiaires sont des preuves historiques du développement, pas un substitut au dernier gel. /tmp/ai-guard-core-green3.log a exécuté1test capture lib ; ses intégrations filtrées à0 ne sont pas comptées.

## Revue et portée restante

Second regard indépendant trusted_runtime_core/invocation_review : FIFO identifié/reproduit/corrigé ; course initiale canonicalisation/open identifiée puis parcours openat composant par composant ajouté après RED. Relecture finale de ce dernier gel terminée : aucun nouveau bloqueur concret identifié sur policy2 et callers. Rapport /tmp/ai-guard-invocation-final-review.md. Le reviewer a également relu le log de régression caller d'adoption en attente,1/1PASS ; il ne revendique pas cette exécution comme sienne. Voir son rapport pour le log exact.

Limites explicites pour AUDIT_FOLLOWUP de l'orchestrateur :

- L'ancrage retire la dépendance aux noms d'ancêtres APRÈS pinning et refuse les redirections par symlink DURANT pinning. Il n'authentifie pas globalement le namespace d'installation avant approbation. Un administrateur pouvant déplacer un véritable arbre owned contrôle depuis un parent writable reste une influence distincte ; aucune suppression globale de l'autorité admin revendiquée.
- Le propriétaire root/contrôle du leaf/fichier peut encore modifier le basename/contenu entre ultime vérification et exec ; ni atomicité contre root/même UID ni protection contre ces writers légitimes n'est annoncée.
- Configurations, plugins et chargeurs natifs sélectionnés par le compte de contrôle restent sa frontière de confiance. PATH helpers système ne constitue pas une médiation de tous les chargements transitifs. Aucun ajout automatique d'un répertoire group-write si un credential helper Desktop manque : erreur/compatibilité à qualifier réellement.
-512MiB+1 est une borne d'octets, pas un timeout dur des I/O régulières bloquées sur NFS/FUSE/disque défaillant. La matrice filesystems/plateformes reste à qualifier.
- Suite globale, build release, Docker/Podman/Linux/macOS réels, supervision après mort broker,100workers et cloud/Kubernetes/PKI : NOT_RUN par ce sous-agent, pris en charge séparément par l'orchestrateur. Aucune release ni déclaration production-ready.

Étude native indépendante préalable : /tmp/ai-guard-invocation-descriptor-check.md et /tmp/ai-guard-descriptor-probe.kMqelu/{probe.rs,result.txt}. Le test prouve fchdir+./basename Darwin et son cwd leaf ; /dev/fd échoue EACCES avec/sans CLOEXEC. Recherche ACL/bornes/sources : /tmp/ai-guard-invocation-acl-read-bounds.md. Référence officielle emplacement Podman : https://github.com/podman-container-tools/podman/blob/main/docs/tutorials/macos_autostart.md. Les API ACL sont étayées par le SDK Apple et les sources Apple citées dans la recherche ; aucune cible externe exécutée.
