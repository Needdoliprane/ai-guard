# Coût des gardiens retenus — protocole passif borné

1 novembre 2026. Analyse de source au checkout `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, HEAD `b32711d`, arbre initial propre. Note privée, mode 0600. Aucune mesure de processus réalisée par son auteur ; aucun test, moteur, probe IPC, signal, nettoyage, credential, capability ou `.env` ouvert. Aucune édition du dépôt.

## Résultat utile et limite

Deux snapshots courts peuvent mesurer le CPU propre et le RSS d'un gardien vivant exactement identifié, et relever ses enfants présents aux bornes. Ils ne mesurent pas tout le coût moteur ni le coût par Arm. La source établit des boucles persistantes ; elle n'établit aucune consommation CPU élevée. Une mesure faible ou une absence d'enfant aux bornes ne démontre pas un coût nul.

Les sources lues sont AGENTS, ARCHITECTURE_CONTRACT, CODING_RULES, PRODUCTION_READINESS, TEST_STRATEGY, les notes `broker-runtime/d5-source/retirement-analysis.md` et `h10/native-interpretation.md`, puis les producteurs enforcer/runtime concernés. Pour les outils macOS, les références primaires locales consultées sont `/usr/share/man/man1/ps.1` et `/usr/share/man/man8/lsof.8`. `/usr/sbin/lsof` est disponible ; aucune de ses commandes d'observation n'a été exécutée.

## Attribution PID → racine sans argv

`ps comm` aide à reconnaître l'exécutable, mais ne distingue pas broker et enforcer du même binaire, ni leurs racines. `spawn_helper` reconstruit l'environnement, utilise le même exécutable et fixe cwd à `/` (`src/broker/enforcer/client.rs:1308–1322`). Les Status/discovery ne portent pas de PID. Aucun lien ne doit être déduit du nom de binaire seul.

Une attribution passive est possible par métadonnées FD :

- `RoleLease` conserve ouvert le fichier `.owner.lock` sous sa racine de rôle, pendant la vie du rôle (`src/broker/discovery.rs:40–70`). Le flock exclusif est pris par `persistent_try_lock` (`src/control_fs/persistent.rs:638–645`).
- `PrivateListener` conserve le listener Unix et son répertoire sur `endpoints/i-<16hex>/s` (`src/broker/local_ipc.rs:182–270`). Aucun besoin de s'y connecter.
- Dans ce harnais, la racine du rôle enforcer est `<QA>/b/enforcer` (`tests/broker_runtime.rs:752`). Pour la racine QA déjà autorisée `/private/tmp/br-euAmXg`, le verrou déterministe est donc `/private/tmp/br-euAmXg/b/enforcer/.owner.lock`. Ne pas appliquer cette composition à une autre forme de racine sans preuve.

Commande proposée, avec le socket EXACT déjà connu d'un inventaire de chemins autorisé :

```sh
/usr/sbin/lsof -nP -F pRcn -- '/private/tmp/br-euAmXg/b/enforcer/.owner.lock' '/private/tmp/br-euAmXg/b/enforcer/endpoints/i-REMPLACER_PAR_NOM_DEJA_CONNU/s'
```

Le deuxième argument est un emplacement à remplacer, pas un glob à exécuter. Aucun contenu d'annonce privée n'est requis. `p/R/c/n` sélectionnent PID, PPID, nom de commande et nom du fichier sélectionné ; `f` peut être émis automatiquement comme séparateur de FD. Seuls les deux chemins sélectionnés sont recevables dans cette sortie. `-nP` supprime les résolutions de noms réseau/services. Aucun `+D`, parcours de répertoire ou inventaire général de FD.

Conserver un PID comme attribué si le même PID correspond aux deux chemins exacts et à un `comm` cohérent. Répéter l'attribution aux deux bornes pour écarter une ancienne association. Un FD ouvert sur le verrou seul ne prouve pas le détenteur exclusif du flock : un candidat transitoire peut l'ouvrir. Plusieurs candidats, chemins ambigus, socket non connu, permission refusée ou résultat incomplet => attribution BLOCKED, pas sélection au jugé. Cette jonction prouve une référence observée à la racine ; elle n'authentifie pas un tenant, un Arm ni un exécutable par hash.

Si le PID a déjà été attribué de cette manière, limiter la vérification aux mêmes chemins et à ce PID :

```sh
/usr/sbin/lsof -nP -a -p PID_EXACT -F pRcn -- 'CHEMIN_VERROU_EXACT' 'CHEMIN_SOCKET_EXACT'
```

Les valeurs en majuscules doivent être remplacées par des valeurs connues ; aucun PID n'est inventé dans cette note. Aucun recours à argv, environnement, `auth`, capability, .env ou connexion IPC pour combler une attribution manquante.

## Deux snapshots scalaires, par Luna séparément

Cible : 20–30 secondes entre T0 et T1, deux bornes seulement, sans relance si le gardien disparaît. Horodater les bornes depuis l'orchestrateur et conserver l'intervalle réellement constaté avec sa précision. Le délai demandé ne remplace pas le délai observé. À chaque borne, prendre les mêmes commandes pour la même liste explicite de PID attribués :

```sh
/bin/ps -ww -p PID_EXACT -o pid=,ppid=,etime=,time=,pcpu=,rss=,comm=
/bin/ps -ww -S -p PID_EXACT -o pid=,ppid=,etime=,time=,pcpu=,rss=,comm=
```

Plusieurs PID déjà attribués peuvent être séparés par des virgules dans `-p`. Le premier appel mesure le temps CPU propre. Le second est facultatif mais utile : sur ce macOS, `-S` ajoute au temps du parent les enfants sortis que le noyau lui attribue. Il aide à voir un coût de CLI brefs que deux instantanés manqueraient. Les deux commandes sont successives : leurs valeurs ne sont pas atomiques ; ne pas soustraire leur écart comme s'il était une mesure exacte des enfants au même instant.

Pour les enfants, `ps` macOS ne documente pas de sélection `--ppid`. Option passive compatible avec les seuls champs autorisés : un inventaire scalaire à chaque borne, puis projection en mémoire des lignes du gardien et de la fermeture descendante par PPID, sans conserver les lignes étrangères :

```sh
/bin/ps -A -ww -o pid=,ppid=,etime=,time=,pcpu=,rss=,comm=
```

Si l'inventaire global est hors du périmètre d'observation retenu, l'omettre et marquer enfants NOT_RUN ; ne pas le remplacer par une recherche d'argv. Si les PID enfants sont déjà connus, préférer leur sélection explicite `-p` avec les mêmes champs. Un enfant peut naître puis disparaître entre les bornes ; un PPID peut changer ; une nouvelle ligne n'est pas nécessairement une nouvelle naissance. L'absence observée est seulement « aucun enfant capturé à cette borne ».

Liste blanche de champs : PID, PPID, etime, time, pcpu/%cpu, rss, comm. `comm` désigne la commande, tandis que `command` et `args` contiennent les arguments selon le manuel. Ne pas employer `ps aux`, `ps -ef`, `command`, `args`, environnement, trace, sampling de stack, ni lsof sans sélection des chemins exacts. Aucun Docker/Podman, socket natif, test ou nouveau gardien n'est nécessaire.

## Interprétation minimale

| Donnée | Conclusion permise | Limite |
|---|---|---|
| `time` sans `-S`, T1−T0 | CPU utilisateur+système accumulé par le même processus pendant l'intervalle observé | Pas CPU des CLI enfants, broker client, daemon/VM moteur ou workers ; résolution d'affichage bornée |
| `100 × ΔCPU / Δtemps` | Pourcentage moyen d'un cœur pour ce PID, dans la précision des deux mesures | Ni charge machine globale ni pic ; plusieurs threads peuvent dépasser un cœur |
| `%cpu` | Indicateur natif de moyenne décroissante sur jusqu'à une minute | Ce n'est pas la moyenne précise de T0→T1 |
| `rss × 1024` | Octets résidents de ce processus à cette borne | Pas pic, pas mémoire réservée des stacks, pas total exclusif ; sommer peut compter des pages partagées |
| `etime`, PID/PPID/comm, attribution FD répétée | Cohérence apparente de l'identité entre les bornes | Pas identifiant kernel de naissance inviolable ; disparition, recul d'etime, changement de comm ou attribution => delta inutilisable |
| `time` avec `-S`, T1−T0 | Variation du compteur parent incluant les enfants sortis attribués | Peut incorporer CPU pré-T0 d'un enfant vivant à T0 puis récolté ; exclut des enfants encore vivants à T1. Ne pas l'étiqueter coût exact de fenêtre ni l'ajouter aux enfants sans correction de frontière |
| Enfants présents aux deux bornes | Delta propre de ces PID si identité cohérente | Cohorte partielle ; pas cadence de spawn ni coût complet des enfants brefs |

Un delta `time=0` signifie seulement « delta inférieur à la résolution observée », sauf preuve de précision suffisante. Conserver les chaînes brutes de `time` ; gérer les éventuels jours/heures/minutes, ne pas les parser comme une simple minute décimale. Pas de mesure par slot à partir du RSS ou du CPU global du gardien.

## Pourquoi ces périmètres sont nécessaires

Le serveur a un thread d'horloge avec sommeil 25 ms (`enforcer/server.rs:101–133`), un acteur par slot avec stack demandée 1 MiB et runtime Tokio courant (`:374–401`), puis boucle 25 ms quand live ou arrêt ciblé suivi de 250 ms quand stopping (`:420–467`). Ce sont des durées/tailles de source, pas du CPU/RSS mesuré. Watch émet Status toutes les 100 ms (`:159–164`, `enforcer.rs:36`) ; le client récupère séquentiellement les slots par paquets de 32 (`enforcer/client.rs:1117–1149`). Une partie du coût est donc dans le client/broker, en dehors du PID enforcer.

L'arrêt ciblé inspecte l'autorité puis le CID ; même un CID arrêté/Created ou absent provoque des CLI selon le chemin atteint (`runtime/managed.rs:558–707`, `:845–849`). Les CLI sont des processus brefs : le RSS du parent ne les inclut pas. Le traitement du moteur Docker Desktop/Podman n'est pas un descendant nécessaire du gardien ; ces snapshots ne l'attribuent pas à une racine.

## Avant un retrait

Ce protocole apporte une preuve de coût, jamais une permission de disarm. D5 reste ouvert : l'arrêt courant, Quarantined, un terminal, un helper nettoyé ou un CID absent ne prouvent pas la fin d'un ancien start. Le coût mesuré ne ferme aucune de ces ambiguïtés.

Avant de retirer une cible, il faut notamment une identité exacte Arm/digest/epoch/génération/CID/claim/émetteur, une révocation ordonnée avec Prepare pour le cas jamais préparé, ou une provenance fiable de complétion normale/fencing irréversible pour le cas émis ; puis l'ordre durable audit/ACK/retrait, replay idempotent et tombstone conservée en reprise. La source actuelle n'a pas de Retire/Disarm et retient toutes les Arm à la reprise. Voir D5 pour ces conditions ; ne pas les réimplémenter ou réinterpréter dans une observation système.

Statuts de cette note : PASS pour l'analyse statique et le protocole ; NOT_RUN pour CPU/RSS/enfants réels, tests et moteurs ; BLOCKED pour toute conclusion « coût total attribué » ou « retrait sûr disponible » sans preuves supplémentaires ; G09 FAIL conservé, aucun gate clos.
