# TODO — fork KoldLab/filmcraft

Idées explorées avec Sami, pas encore commencées. Chaque item reste 100 % Rust.

## Transcription — bug « Transcribe ne fait rien » (prioritaire)

Constaté le 2026-10-12 : clic sur Texte ▸ Transcript ▸ Transcribe → rien ne se passe.
- Cause : aucun modèle Whisper installé ; `transcript.generate` échoue (`NotInstalled`) et l'erreur
  part seulement dans la barre d'état (`app.ui.status`, `crates/ui-egui/src/panels/text.rs` `run`).
  Aucune UI n'appelle `transcript.downloadModel`.
- Préférences actuelles (`%APPDATA%\FilmCraft\preferences.json`, `mediaAnalysis`) : langue `en`,
  détection auto off, modèle `whisper-base` → le français serait transcrit comme de l'anglais.

À faire :
1. Déblocage : télécharger `whisper-small` (~985 Mo, huggingface.co/openai/whisper-small, SHA-256
   vérifié) — demander l'OK de Sami avant — et régler `defaultLanguage: "fr"`,
   `whisperModel: "whisper-small"`.
2. Si le modèle manque au clic sur Transcribe : dialogue « Télécharger <modèle> (taille) ? » avec
   barre de progression.
3. Afficher les erreurs de transcription dans le panneau, pas seulement dans la barre d'état.
4. Vérifier si `transcript.generate` tourne sur le fil de l'UI (il semble que oui → appli figée
   pendant des minutes) ; si oui, le passer en tâche de fond avec progression et annulation.
5. Défaut de langue `fr` pour ce fork (réglage, pas constante).

## Montage sur la musique

### Marquer les beats (`markers.markBeats`) — ~½ à 1 jour
Sélectionner la musique dans la timeline → Marqueurs ▸ Marquer les beats… → un marqueur de
séquence sur chaque temps détecté. Le détecteur existe déjà : `crates/audio-dsp/src/remix.rs`
(`analyze` : tempo 50–200 BPM + grille de beats), utilisé seulement par Clip ▸ Remix.

Options :
- Fréquence : chaque temps / tous les 2 / toutes les mesures (4) / toutes les 2 mesures.
  Défaut proposé : **toutes les mesures** (à 120 BPM = un marqueur toutes les 2 s). À confirmer.
- Plage : toute la musique / entre In et Out.
- Décalage : ± quelques images (couper juste avant le temps).
- Couleur de marqueur dédiée.

Flux complet : marquer les beats → sélectionner les clips → Clip ▸ Automate to Sequence…
avec `placement: unnumberedMarkers` (existe déjà, `crates/engine/src/project_tools.rs`).

### Plus tard
- Couper sur les temps forts (drops, caisse claire) via l'enveloppe d'onsets plutôt qu'une grille.
- « Caler la coupe la plus proche sur le beat » pour ajuster à la main.

## Transcription et montage auto (remplacer KoldVideoWorkflow)

Objectif : abandonner `C:\Users\Sami\KoldVideoWorkflow` (Python, GPU) et tout faire dans
FilmCraft. Pas de CUDA : processeur d'abord, GPU seulement via wgpu.

1. Ajouter `whisper-large-v3-turbo` et CrisperWhisper turbo (verbatim, garde les « euh ») au
   catalogue (`crates/speech/src/models.rs`), sur processeur. Mesurer le temps réel sur un rush.
2. Importer les `.json` Kold existants (à côté des rushes ou dans `transcripts/`) via
   `transcript.set`, pour ne pas retranscrire les ~157 capsules.
3. Panneau Transcription façon Kold : hésitations colorées, pauses, choix de « ma voix »,
   recherche dans toutes les transcriptions du projet.
4. Porter `KoldVideoWorkflow/backend/montage.py` en commande `montage.matchScript` : coller un
   script → prises candidates par phrase → nouvelle séquence versionnée (`Capsule_AUTO_v1`),
   jamais dans une séquence existante.
5. Plus tard : transcription GPU via wgpu (burn), analyse visuelle (remplaçant Marlin-2B).

Questions ouvertes : laisser tomber la vision pour l'instant ? Modèle par défaut CrisperWhisper
turbo ou turbo standard ?
