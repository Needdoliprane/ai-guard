# Candidat de démarrage humain possédé

Base : `6cc6b3a25f4dd1fea4ead3180cde1d58fb912001`. Ce changement réalise la verticale W0 R2 relue : startup réel complet, découverte de candidats bornée, identité/ressources locales sur la propre source, fermeture explicite des autres routes modernes et ownership serveur jusqu'aux travaux acceptés/en queue et à leur cleanup.

Les nouvelles régressions sont dans `api/human_sources/tests.rs`, `audit/human_modern/tests.rs`, `human_authority/startup/tests.rs` et les tests de digest public sous `gateway_config/human_sources/`. Elles utilisent de vrais SQLite privés migrés par le coordinator existant ; aucun UUID, candidat ou flag de disponibilité ne remplace une preuve de source. Les tests A/B gardent mutex ou `BEGIN IMMEDIATE` A jusqu'à identité B authentifiée, contexte opaque et reçu/floor de ressources observés durablement par connexion indépendante. L'observateur Store voisin doit rester vide. Busy propre et restauration du timeout ont leurs tests distincts.

Exécution : **NOT_RUN**. Cet agent n'a exécuté ni Cargo/rustc/binaire projet, ni moteur, fournisseur, IdP ou cluster. Rustfmt de confiance ciblé et `git diff --check` sont seulement des vérifications statiques. Compilation, assertions logicielles, revue indépendante, Docker/Podman/Kubernetes et qualification production restent à établir sur un gel identifié. Aucun PASS historique n'est transféré, aucun fingerprint d'audit ni gate G01–G12 n'est clos.

Modes legacy et dispatch Full3 sont conservés. Le nouveau mode ne réalise ni provisionnement hors ligne manifeste/enveloppe, ni découverte fraîche cross-source, ni sessions/révocation OIDC modernes, ni approbation/lancement humain, ni permission egress ou filtrage réseau. Le [guide opérateur synchronisé](../../../../setup/identity/OWNED_SOURCES.fr.md) décrit les vrais flags et les refus/reprises. Revue source/conception et éventuelles preuves d'exécution restent distinctes.

## Validation logicielle ultérieure — 13 novembre 2026

Les paragraphes précédents conservent la source initiale, l'activité de l'agent
et le statut `NOT_RUN` de l'implémentation. La [validation ultérieure du parcours opérateur et du startup owned](../human-sources-operator/README.fr.md)
consigne 107 fonctions nommées uniques PASS sur la source figée
`dbee1d5f90abd884badf5cd255b69c436f38ae4a`, en deux campagnes de 34 et 73,
avec revue indépendante des résultats. Elle inclut le producteur opérateur
ajouté après ce registre W0. Sources, builds, échecs de harnais conservés et
limites figurent dans ce rapport et [RESULTS.json](../human-sources-operator/RESULTS.json).
Cet ajout ne réécrit pas les preuves initiales et ne qualifie ni autre checkout,
coupure électrique physique, worker métier/runtime, IdP, fournisseur, frontend,
release ou gate de production. Le profil délibéré à deux GET et ses exigences
obligatoires restantes restent explicites dans les guides opérateur actuels.
