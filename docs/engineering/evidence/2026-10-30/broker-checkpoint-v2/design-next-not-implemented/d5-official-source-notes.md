# D5 — piste officielle, pas qualification du moteur testé

30 octobre 2026, root. Consultation publique en lecture seule ; aucune exécution worker, destruction, installation ou action cloud.

Le daemon local répond à `docker version --format '{{json .Server.Components}}'` : Engine29.9.2, API1.56, GitCommit court0367cbe, Linux/ARM64, build20octobre2026 ; containerd2.4.2, runc1.5.2. La commande utilise le socket Docker Desktop déjà autorisé. Cette métadonnée ne vérifie pas l'absence de patches dans le moteur distribué.

Piste dans la branche master publique consultée : [start.go](https://github.com/moby/moby/blob/master/daemon/start.go) prend le verrou du conteneur puis refuse les états RemovalInProgress et Dead. [delete.go](https://github.com/moby/moby/blob/master/daemon/delete.go) pose un état de suppression en cours, arrête le processus, marque Dead et retire les métadonnées. Ces chemins suggèrent d'étudier leur ordre commun comme candidat de fence exactCID ; ceci est une inférence de lecture, pas une garantie de concurrence ou de reprise.

Les deux URLs raw essayées avec le commit court déclaré (`https://raw.githubusercontent.com/moby/moby/0367cbe/daemon/start.go` et même chemin delete.go) répondent404. La correspondance avec le binaire testé n'est donc pas établie. La branche master ne qualifie ni ce moteur29.9.2 ni Podman. Il faut obtenir les sources exactes de la distribution, examiner verrouillage/retours d'erreur/persistance sous panne et effectuer les essais de course séparant start retardé avant effet et réponse perdue après effet.

Même si un fence exactCID était établi, les DTO actuels ne clôturent pas l'usage inconnu après suppression préterminale. Conserver l'incertitude, le claim et le quota tant que le contrat de terminal/usage ne dispose pas de faits suffisants. Aucun nouveau droit de suppression préterminale ou de libération n'est livré par cette note.

Statuts : recherche de piste **PASS en lecture**, correspondance source/binaire et qualification de fence **NOT_RUN/non établies**, tests runtime **NOT_RUN**. Le checkpoint V2 en cours n'en dépend pas et ne prétend fermer ni D5 ni G09.
