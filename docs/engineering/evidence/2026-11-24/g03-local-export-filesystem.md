# G03 · Publication concurrente et volume plein sur macOS

Source testée `b736444a871137aff40b1f663696e02ecdde07cf`, arbre
`e78c36d2d605905f7281efda0a126031f7ad4ace`. Le conducteur Rust de
recette appelle `workspace::extract_workspace` directement. Il est
lié à la bibliothèque sélectionnée par le build Cargo hors ligne du
test `workspace_export` sur cette source. La recette utilise le
système de fichiers local, des archives synthétiques et aucun moteur.

| Cas | Observation |
|---|---|
| Deux publications vers le même nom neuf | Les deux lecteurs franchissent ensemble la première lecture de l’archive. Une publication réussit et l’autre échoue. La destination contient intégralement les octets du gagnant. Aucun staging ne reste et l’original est intact. |
| Volume APFS borné | Une image UDRW de 256 Mio laisse 9 728 Kio disponibles avant la tentative d’écriture de 16 Mio. L’export reçoit l’erreur d’E/S native 28 (`ENOSPC`). La destination et le staging sont absents après le refus. L’original est intact. |
| Rejeu du journal d’export | Deux tests ciblés passent. La perte de réponse d’audit restitue le même reçu après réouverture. La limite du journal préserve l’export publié sans produire de reçu et refuse de republier. |

Le premier montage APFS ignorait les propriétaires. Le garde d’export
a refusé avant l’écriture. Le rejeu a monté la même image avec la
gestion des propriétaires rétablie, puis a vérifié son détachement.
L’image jetable a été retirée.

Les pièces privées sont sous `.git/qa-evidence-retired/g03-fs-b736444a/`
dans le checkout de coordination, avec leur fichier `SHA256SUMS`
vérifié. SHA-256 du conducteur source
`be5cdfdc9fafc877102bfc336df66ab92a254c3c1dfd83fd22325627c336140a`.
SHA-256 du journal des deux tests
`6542952149c796b51a32bd3dfe14d06115dbb33204cf80066c4d532459149efd`.

Les pannes d’écriture après publication, les erreurs de synchronisation
et la qualification de la matrice sur moteur gardent leurs propres verdicts.
