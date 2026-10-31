# Revue indépendante ciblée — ascendance de l’export

31 octobre 2026. Lecture seule dans /Users/louis/Documents/ai-guard/.worktrees/broker-service. Périmètre : changements de src/workspace/authorized.rs et src/control_fs.rs, articulation des capacités avec l’export ; aucun jugement d’approbateur sur mon propre workspace.rs. Consignes AGENTS, RTK, ARCHITECTURE_CONTRACT, CODING_RULES et TEST_STRATEGY prises en compte. Skill security-audit employé comme guide de revue ciblée, sans campagne d’exécution. Aucune source, test ou binaire modifié ; aucune commande Cargo, moteur, fournisseur ou credential utilisé.

Résultat de lecture : aucun défaut bloquant concret établi dans ce périmètre et sur ces empreintes. Cette conclusion porte sur les contrôles décrits ci-dessous, pas sur une qualification de runtime ou de stockage.

- authorized.rs:105–156 : revalidate recoupe root, noms relatifs, identités retenues/courantes et parent physique « .. ». check_export_ancestry entoure la vérification des droits de TOUTE la chaîne par ces recoupements. Un ancêtre au-dessus de la racine allowlist n’est pas omis.
- control_fs.rs:559–592 : le nouveau point d’entrée délègue à check_directory(false), sans chmod, suppression d’ACL, création ni autre réparation. Dossier réel, filesystem pris en charge, propriétaire root/euid et absence de droits groupe/autres en écriture sont contrôlés. L’exception sticky demeure réservée à un dossier appartenant à root, conformément au contrat préexistant ; elle n’autorise pas un ancêtre arbitraire appartenant à un autre UID.
- control_fs.rs:777–909 : le contrôle réemploie la politique ACL existante. Linux repose sur les bits de masque ACL des seuls filesystems locaux admis ; Darwin inspecte le FD et refuse les droits mutateurs/semantiques inconnues. Aucune nouvelle exception export ne contourne cette politique.
- authorized.rs:183–246 : ExportAncestry est opaque, non sérialisable, sans Clone public. Le chemin legacy normalise puis ouvre la chaîne une fois. from_retained clone seulement les FD, identités, composants et boundary déjà retenus, avec vérification avant/après ; aucune résolution de pathname ne choisit une nouvelle autorité de création. Les ouvertures initiales sont DIRECTORY/NOFOLLOW/CLOEXEC/NONBLOCK (:23–24,426–442), les copies utilisent File::try_clone. path() sert à la provenance.
- authorized.rs:321–372 : l’autorisation export conserve la séparation lexicale/physique des racines et la revalidation des politiques. L’extract passe la capacité retenue à Staging. Les sources restent des données non fiables : AuthorizedSource::revalidate n’a pas reçu de contrainte de permissions de contrôle ; une source mode0777 ne devient pas une autorité d’écriture dans la destination.

Couverture de tests lue, non exécutée par ce relecteur : cinq refus de permissions initiaux/nouveaux droits, deux contrôles positifs (parent privé/non mutable ; source mutable admise), dans authorized_workspace_tests:529–625. Les régressions préexistantes de substitution/allowlist demeurent présentes. Luna dispose du gel et exécute les vérifications ; leurs résultats ne sont pas anticipés ici. La preuve d’exécution est à rattacher à ses logs, pas à ce rapport de lecture.

Limites exactes : droits POSIX/ACL et topologie des filesystems pris en charge, pas isolation contre le compte de contrôle lui-même, root, administrateur stockage ou montage privilégié concurrent. Les contrôles observent des points de revalidation ; ils ne promettent pas un snapshot atomique de mutations privilégiées. Pas d’essai réel d’un UID adverse, d’ACL Darwin mutatrice, de reprise après perte d’alimentation ou de matériel de stockage dans cette revue. Les FD clones, le bootstrap et les runtimes n’ont pas été exécutés ici. Durabilité d’ancêtres externes récemment créés et fenêtre publication FS→naissance outbox restent les limites déjà documentées, sans nouveau succès fictif.

Commandes de contrôle utilisées : git diff sur les deux fichiers, lectures numérotées ciblées, rg des points ACL/permissions/fixtures, shasum -a 256 avant/après. Empreintes identiques en fin de revue :

```text
b5b8a2002330f130129a872b198cc37675e53932f449b5d852298351d731d1d2  src/workspace/authorized.rs
275a4df60c2fca305eddcf668ef3d28548bbf0e0162207d11b01cf615dc4235f  src/control_fs.rs
```
