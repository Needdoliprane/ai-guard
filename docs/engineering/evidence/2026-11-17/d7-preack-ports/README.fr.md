# Ports D7 avant ACK — candidat source, 17 novembre 2026

Ce lot borné configuration/client/exécuteur appartient à l’option A approuvée.
Commit source : `f73e9a4895cd160f5158ced944c39c7d6959ecca` (sept chemins Rust).
Il doit être composé avec l’admission native et les propriétaires STOP. Il ne
constitue pas un correctif utilisable séparément. Compilation, corps des
régressions et revue indépendante sont **NOT_RUN** sur cette source. Aucun
constat D7, gate de production ou garantie runtime n’est clos. [English](README.md).

Les registres privés V1 et V2 acceptent `max_recovery_stop_owners`, avec défaut
zéro et borne 1024, indépendamment de `max_enforcer_slots` (S). W est transmis
dès la genèse Full3 fraîche et aux rôles internes de reprise/existants. Les
wrappers compatibles passent W=0. Pour W>0, une cellule native est réservée au
rattrapage ; W-1 cellules supplémentaires au plus acceptent durablement des
propriétaires STOP. Le maximum déclaré est S+W contextes, pour au plus S+W-1
cibles couvertes de façon permanente. W=0 n’ajoute aucun contexte ni capacité
de rattrapage. La disponibilité exige encore l’installation native réelle et
des preuves distinctes attachées à leur source.

La cible prospective est un type strict distinct sans reçu Arm.
`RegistrationV1` exige et valide toujours le vrai reçu. La réponse native
authentifiée lie epoch, génération d’admission, génération d’annulation, nonce
et digest de la cible immuable. Le pending Arm exact devient durable avant
Reserve. Une capacité process opaque vient seulement de la première transition
native authentifiée `NotIssued → MayHaveIssued` ; Bind revérifie son nonce.
Une réponse perdue ou un rejeu ne la recréent jamais. La rétention cliente survit à l’abandon du
waiter et à l’échange Arm inconnu. La liaison ne change ni identité Arm,
snapshot ni échéance. Un ancien Arm pending rejoint seulement son propriétaire
natif existant et reçoit STOP/ObserveOnly, sans nouvelle revendication.

**Écart de compatibilité : la sélection Legacy par défaut ne possède aucune
factory native prospective.** Le candidat refuse explicitement un Arm Legacy
frais avant son échange HTTP. Les cibles Legacy déjà acquittées conservent
leurs voies STOP/reprise. Aucune bascule automatique, suppression ou migration
n’a lieu. Un vrai port prospectif V1 ou une migration explicite relue séparément
préservant les données reste nécessaire avant d’annoncer une livraison
équivalente du parcours par défaut. Un rôle Full3 frais est une création
explicite de rôle neuf, pas une procédure de migration d’installation existante.

Les régressions source de l’exécuteur utilisent vrais journal/writer, Gateway,
SQLite et IPC natif, avec métadonnées Created synthétiques et fixture native
SyntheticReady. `/usr/bin/true` est épinglé seulement comme donnée
d’invocation ; aucun moteur ni fournisseur ne s’exécute. Les refus vérifient
zéro requête HTTP Arm/claim. Legacy et annulation gardent une tête inchangée ;
le refus de capacité garde un seul pending durable, avec la même identité et
échéance au retry. Le cas ACK pending
vérifie un rejeu Arm idempotent, l’identité/échéance/input d’origine, l’état
worker inchangé et l’absence de claim. Les racines privées des fixtures sont
conservées : Drop du writer ne prouve pas son retour natif.

Le cas writer bloque le vrai acteur après son appel Journal puis injecte un
acquittement inconnu avant Reserve. Un latch client négatif refuse ensuite de
nouvelles identités. Une lecture stricte reste UNKNOWN ou garde cet unique
pending exact ; le retry n’envoie aucun Arm/claim. Ce latch ne prouve pas une
transition durable de reprise entre processus.

La disponibilité après refus de capacité ou publication pending inconnue reste
**OPEN**. Pending DATA sans vrai propriétaire natif préexistant ne permet pas
une nouvelle allocation Original au retry ; aucun quota n’est libéré. Une
reprise contrôlée exige une continuation distincte relue prouvant le
non-engagement natif. Aucun pool caché d’intérêts propriétaires, nouvel Arm
automatique ou migration implicite n’est ajouté. Ces limites interdisent de
présenter le candidat comme une remédiation D7 complète.

Seuls rustfmt et `git diff --check` ont été exécutés sur ce lot (RC0). Voir
l’[état typé](TESTS.json). Docker, Podman, Kubernetes, fournisseurs, mesures de
ressources, latence et qualification de bout en bout restent **NOT_RUN**.
