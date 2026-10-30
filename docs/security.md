# Sécurité : mécanismes, périmètres et vérifications

## Modèle de menace

Les entrées d'un agent, son stdout/stderr, ses sorties JSON et le projet importé sont non fiables. Le poste, l'utilisateur opérateur, le binaire runner, la politique, les images choisies et le moteur de conteneurs appartiennent au domaine de confiance. Un administrateur du poste ou du moteur peut modifier ce domaine ; la solution ne prétend pas le neutraliser.

Objectifs : protéger le projet original et les fichiers personnels, empêcher l'agent d'obtenir une autorité de runtime, conserver des traces indépendantes du worker, refuser un lancement sans autorisation tracée, distinguer déclarations et observations. Les failles noyau, du runtime et des images nécessitent leur propre maintenance et éventuellement une VM dédiée.

## Contrôles

| Contrôle | Mécanisme réel | Périmètre | Comment vérifier |
|---|---|---|---|
| Auth API | Deux secrets aléatoires distincts ; Bearer ; comparaison de hashes sans sortie anticipée | Viewer lit, runner ingère/demande une décision | Tests `api_security` ; POST avec viewer retourne 403 |
| Auth IHM | Token en mémoire navigateur ; absence de cookie/session automatique | Lecture locale, pas d'action administrative dans l'IHM | Tests navigateur ; ni URL ni stockage local ne contiennent le token |
| Secrets de contrôle | Originaux `0600`, projections de montage distinctes `0444` sous parent `0700` ; sources privées jamais montées | Gateway reçoit runner/viewer ; dashboard reçoit viewer ; worker ne reçoit aucun de ces secrets | Tests `runtime_operations` et exclusion `mounted-secrets` dans `runtime_security` ; qualification des montages sur moteur réel |
| Ports | Publication explicite `127.0.0.1:4000` et `127.0.0.1:8787` | Hôte local | Inspect Compose ; vérifier écoute avec les outils de l'OS |
| TLS | HTTPS avec rustls pour Jev ; refus des URL HTTP distantes et des redirections | Appel externe Jev | Tests client ; vérifier certificat de l'endpoint réel |
| TLS / mTLS interne | Non activés sur la stack locale ; services dans un réseau de conteneurs dédié | Loopback hôte et réseau interne | Aucun claim de chiffrement de ce tronçon ; exposition distante nécessite une architecture TLS/auth dédiée |
| Droits worker | UID non privilégié, all capabilities dropped, no-new-privileges, read-only root | Processus et descendants du worker | `inspect`, tests des arguments et essai réel de `/proc/self/status` |
| Isolation moteur | Aucun socket Docker/Podman monté ; aucun mode privileged/host PID/host network | Tous les conteneurs applicatifs | Examiner mounts, cap_add, privileged et network_mode |
| Isolation projet | Volume de session, copie filtrée ; pas de bind du dépôt original | Entrées/sorties du worker | Un marqueur modifié dans le worker ne modifie pas l'original |
| Chemins | Refus de traversal, liens et entrées spéciales ; export attaché à des descripteurs de répertoire | Import/export d'archives et substitution concurrente de parent | Tests `runtime_security` et `runtime_races` |
| Réseau worker | none par défaut ; bridge uniquement en observe et sur demande | Tous les processus du worker | Essai HTTPS échoue en none ; bridge est annoncé comme non filtré |
| Ressources | Limites CPU, RAM, PID, tmpfs et durée de job | Worker | Inspect + tests de saturation dédiés sur moteur réel |
| Journal indépendant | Volume audit seulement dans gateway ; identité runner jamais confiée au worker | Audit central | Inspect mounts + tentative d'accès depuis worker |
| Masquage | Valeurs de secrets connues, champs sensibles, formats et options reconnus | Avant stockage/diffusion ; règles également sur commandes textuelles | Tests secrets et fixtures ; les secrets arbitraires inconnus ne sont pas détectables universellement |
| Sorties | Capture stdout/stderr facultative et bornée | Réduction de volumétrie et exposition accidentelle | Tests de bornage ; préférer capture désactivée en usage courant |
| Front non fiable | Données rendues comme texte ; CSP ; pas de scripts/HTML issus des logs | XSS et injection d'interface | Tests navigateur avec payloads HTML/JS |
| Audit saturé | Plafond d'événements/payloads et refus des nouvelles traces | Fail-closed avant lancement | Test quota puis demande suivante refusée |
| Intégrité | Chaînage local, vérification au démarrage, verrouillage après corruption détectée | Détection d'altération partielle | Modifier une ligne d'une copie de test ; intégrité invalide puis refus d'écrire |

## Ce qu'une politique n'assure pas

Une regex sur `rm` n'est pas une sandbox. Un interpréteur peut effectuer la même opération autrement. L'allowlist stricte porte donc sur des invocations exactes et ne s'appuie pas sur une liste de mots interdits comme seule frontière. `review` et `observe` autorisent davantage de programmes ; leurs effets restent dans le worker, mais ne sont pas tous réévalués individuellement.

Les hooks des agents améliorent la visibilité et peuvent influencer leurs permissions. Ils sont versionnés, partiels et parfois désactivables. Les parseurs de sortie fournis ici n'interceptent aucun syscall et ne bloquent pas une action passée. `compatibility.md` expose précisément ces limites.

## Secrets de fournisseurs

Le worker ne reçoit pas automatiquement les identifiants du poste. Un fichier de variables explicitement fourni est limité au parcours cloud déclaré ; les valeurs connues sont masquées localement avant publication des traces. Le moteur et l'opérateur de confiance peuvent lire la configuration du conteneur : ce mécanisme n'est pas un coffre contre l'administrateur du runtime. Une évolution possible consiste à fournir des identifiants courts via un proxy de fournisseur externe au worker.

Éviter de placer des secrets dans les arguments, le prompt ou des fichiers du projet. Le masqueur réduit les fuites usuelles ; il ne peut reconnaître n'importe quelle chaîne sensible inconnue. Le fichier de secrets du projet est exclu de l'archive source distribuée.

## Rétention et reprise

La version courante préfère arrêter de nouvelles exécutions à supprimer silencieusement des preuves. Le plafond d'événements est configurable au démarrage ; la limite de payloads est documentée. Exporter JSONL, arrêter le collecteur, sauvegarder son volume de manière cohérente, puis démarrer un nouveau stockage selon une procédure opérateur contrôlée. Ne pas retirer des lignes d'une chaîne existante et prétendre que son intégrité est inchangée.

En cas de corruption, conserver une copie pour diagnostic. Ne pas régénérer des clés, supprimer des sessions ou recréer la base automatiquement. Le redémarrage du dashboard est sans effet sur la persistance de l'audit. Une panne de disque ou une écriture refusée n'est pas récupérée par un fallback non journalisé.
