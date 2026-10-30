# Second regard final — invocation du moteur et callers

29 octobre2026. Lecture indépendante ciblée, hors audit formel. Périmètre : src/runtime.rs, runtime/{invocation,managed}.rs ; src/operations.rs, runner.rs, runner/managed.rs ; régressions runtime_managed/runtime_program/runner_recovery. Aucun fichier du dépôt modifié par cette revue. Aucun moteur réel, helper ou cible exécuté dans cette passe finale ; les résultats ci-dessous sont les logs des implémenteurs lus par le reviewer.

## Conclusion

Aucun nouveau bloqueur concret identifié sur le code gelé ci-dessous, dans la portée annoncée : sélection du CLI hors PATH/projet, exécution dans le leaf ancré, refus des ACL/modes leaf non admis, invocation durable et adoption auditée, endpoint Docker figé sur les opérations. Cela ne constitue ni une qualification de release ni une fermeture générale de l'autorité des administrateurs d'installation.

## Défauts traités et contrôles relus

- **P2 FIFO fermé** : ouverture nonblocking avant fstat et refus régulier/exécutable. La régression avait reproduit le blocage avant correction ; GREEN11/11 dans /tmp/ai-guard-core-green6.log.
- **Substitution pathname après vérification fermée pour le programme principal** : Runtime conserve un FD du répertoire final ; fichier lu par openat sans suivre de lien ; enfant fchdir sur ce même FD puis exec './basename'. Les contrôles owner root/contrôle, mode022=0 et identité portent sur les objets ouverts, non une réouverture aveugle du pathname. Voir src/runtime/invocation.rs:214,256,292 et src/runtime.rs:475.
- **Course par symlink au pinning initial fermée** : open_directory part du FD '/' et ouvre chaque composant via openat O_DIRECTORY|O_NOFOLLOW. Un remplacement d'ancêtre par lien vers le projet est refusé avant approbation de son identité. RED /tmp/ai-guard-core-pinning-red.log ; GREEN unitaire dans /tmp/ai-guard-core-anchor-unit-final.log.
- **ACL macOS et lecture non bornée fermées dans cette politique** : ACL étendue non vide refusée sur leaf file et leaf directory, API Darwin fd avec conventions correctes ; NULL/ENOENT absence distingué des autres erreurs. Hash512MiB maximum, lecture initial_length+1, refus croissance/troncature et métadonnées avant/après. Linux : la garantie écrite reste celle des POSIX ACL sur filesystem local qualifié, sans extrapolation NFSv4/SMB/FUSE. Voir invocation.rs:311,341,401. Triple RED observé puis GREEN3/3 /tmp/ai-guard-core-anchor-green.log ;5 tests lib PASS dans /tmp/ai-guard-core-anchor-unit-final.log.
- **Environnement** : env_clear, HOME compte OS, configurations standards et aucun sélecteur/loader ambiant ; PATH limité à /usr/bin:/bin:/usr/sbin:/sbin avec répertoires système inspectés. Aucun programme ou mode test ajouté au produit. Cwd devient le répertoire physique approuvé ; sources/build/Compose sont absolus.
- **Docker opérateur** : endpoint Unix résolu une seule fois, EngineID conservé dans ManagedRuntime ; réouverture/inspection en lecture seule avant chaque version/build/compose. --host est placé avant la sous-commande, y compris compose. Le second build vérifie à nouveau l'identité. Podman opérateur ne bootstrappe aucun marqueur ; cette voie raw n'obtient pas la liaison moteur complète Docker par ce changement. Voir operations.rs:459,498,814. Tests opérateur PASS8/8 dans /tmp/ai-guard-operator-binding-final-tests.log et /tmp/ai-guard-caller-policy2-final-tests.log.
- **Adoption** : None et Some policy1/invalide ou programme changé restent lisibles ; seule une option opérateur explicite propose une nouvelle invocation. Vérification readonly du même moteur/marker, ancien binding préservé dans invocation-history, événement stable old/newfp, audit durable puis seed et checkpoint avant retour. Fingerprint moteur des handles inchangé ; aucun claim réarmé. Les shortcuts ancien programme refusent un invocation_event en attente, empêchant un repli après audit503. Voir runner/managed.rs:723.12 recovery tests PASS avant dernier garde dans /tmp/ai-guard-caller-policy2-final-tests.log ; régression dédiée pending_program_change PASS1/1, zéro ignoré, dans /tmp/ai-guard-caller-pending-adoption-green.log (log final lu après gel).
- Erreurs moteur typées et redactions privées relues ; test de capture opaque conservé PASS.

## Limites à conserver publiquement

- Un déplacement d'un **vrai arbre** appartenant au contrôle depuis un namespace modifiable vers celui d'une installation n'est pas la substitution par symlink testée. Sans provenance physique complète du projet/installation, ne pas annoncer la suppression de toute influence administrative au stade de première approbation. Cette limite source a été transmise et acceptée par l'orchestrateur ; aucune exploitation de ce cas plus fort n'a été exécutée.
- Le FD protège le répertoire/CLI principal ; il ne pinne pas toutes les dépendances, configurations, credential helpers ou plugins chargés ensuite par Docker/Podman. Le PATH système retire l'exception implicite Homebrew admin775 mais la compatibilité des helpers Desktop/Homebrew doit être qualifiée sur le parcours réel.
- Root, le même UID de contrôle, writers antérieurs, remplacements administratifs du moteur et propriétés non qualifiées des filesystems restent hors des garanties démontrées. Vérification EngineID et effet utilisent deux connexions : aucune atomicité de fencing moteur n'est établie.
- Le plafond de lecture borne les octets, pas un read bloqué par panne disque/FUSE/NFS. Aucun timeout fort de lecture n'est démontré.
- Docker/Podman réels, matrice Linux/macOS, reprise runtime, capacité100 et qualification globale restent à l'orchestrateur. Aucun test ignoré compté PASS ici.

Recherche et sources officielles détaillées : /tmp/ai-guard-invocation-acl-read-bounds.md.

## Empreintes du gel relu

- src/runtime.rs : d3e74de949482ca200595938485361fd5324f61d5d61e031cfaef1edf4ca722e
- src/runtime/invocation.rs :41c55cc10b1cd29d77c353542d757e01c1df64db785886ec4e6751c6c46db039
- src/runtime/managed.rs :bb5989cf501a76cb3b97c7afb6e4f27887a6f89fa36899ee4facc9e120c873e6
- src/operations.rs :0b5030d4b8c3cd8453b2e21b2c1c8155d0aad2ec491b119fb9ccf4582afb6fc2
- src/runner.rs :1558d6d7507ee58fc2efe72a56f59f2c6ced3112076d0312a2e0d7202912e385
- src/runner/managed.rs :814cd16214aaf7618838d6010a61115162fff61127f7fba00b785b99168c9058
