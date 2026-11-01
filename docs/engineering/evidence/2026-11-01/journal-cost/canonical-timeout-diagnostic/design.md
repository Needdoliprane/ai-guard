# Diagnostic temporaire Io outbox512

Périmètre autorisé : ClientState.receive et branche Command::EnqueueEvents dans src/broker/writer.rs ; seul test wrong_context_secrets_strict_nested_decision_and_event_cap_are_rejected_without_writes dans tests/broker_outbox.rs. Aucun Cargo/test lancé par l'implémenteur, aucune lecture H10/credential/moteur, aucun autre source édité pour ce diagnostic. Rustfmt ciblé et git diff --check RC0.

SHA AVANT (copies privées conservées à côté de cette note) :

- writer.original.rs / src/broker/writer.rs : 47c72983851dc057c22d6731f55b99b0c208bbe8401d8333e00e582d217538d7
- broker_outbox.phase2.rs / tests/broker_outbox.rs : f9a34cfc7fbf3398bc81821af21860ce8958a0a36a4e2dc376c7faf1481ac7a5

SHA TEMPORAIRES gelés :

- src/broker/writer.rs : 60d33e9fadde129634e8e08789273fa628452039908db7f02eb4915e2282be01
- tests/broker_outbox.rs : 8fa739261f1b6a0cc215fe0c7bf50bba25739904c80c0630af2702facaafe956

Les fichiers writer.temporary.patch et broker_outbox.temporary.patch contiennent uniquement le delta temporaire par rapport à ces copies, mode0600. Ils ne comprennent pas la correction canonique permanente. Les diff retournent naturellement RC1 car les fichiers diffèrent.

Instrumentation fermée :

1. receive conserve exactement timeout(self.timeout), l'ordre de ses deux conversions d'erreur vers Io et le Result interne. Trois marqueurs après résultat distinguent timeout, channel_closed, backend_io ; aucune donnée d'appel ni erreur libre n'est publiée. Aucun timeout changé.
2. Command::EnqueueEvents exécute le même backend une seule fois et envoie son même Result une seule fois. Après la tentative send, il publie seulement result_ok, item_count Option, delivered et elapsed_ns. delivered indique l'acceptation par le oneshot à cet instant, pas une lecture caller ni un ACK central. Ce marqueur distingue une completion backend effectivement observée d'une absence de preuve.
3. Après les deux acceptations déjà prévues, le test conserve son TempDir avant l'appel512, sans afficher son chemin. Il mesure cet appel. Sur erreur seulement, il fait une unique lecture Journal.load et publie uniquement ok/head/events/durée avec observation=after_enqueue_error ; l'original unwrap reste ensuite en place et conserve le FAIL. Sur succès, tous les oracles d'origine se poursuivent.

La lecture après erreur peut attendre le mutex ou observer head0 pendant une préparation backend hors mutex : elle ne prouve ni head au timeout ni completion backend. Elle a ses propres synchronisations et se nomme explicitement comme observation postérieure. Aucune mutation/retry/polling/attente artificielle ajoutée. Si le processus finit avant le marqueur backend, sa completion reste inconnue. Un marqueur timeout prouve seulement la branche timeout du caller de cette reproduction ; il ne prouve aucune causalité de la factorisation.

Une seule reproduction proposée à Luna, mêmes budgets et même profil debug, test exact inchangé. Après conservation de ses sources/logs/RC et instruction Root, retirer exclusivement ces deltas : writer doit retrouver SHA47c72983…7538d7 et outbox SHAphase2 f9a34cfc…1ac7a5. Vérifier le gel courant avant retrait afin de préserver tout changement concurrent. Les copies privées donnent les bytes exacts de référence, sans git checkout/reset global.
