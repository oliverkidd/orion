# DICTATION end to end with a recording in place of the microphone: the helper transcribes it and the
# words land in the box as a paste (ORION_DICTATION_FILE, src/dictation.rs).
say -o "$WORK/talk.wav" --data-format=LEI16@16000 "then run the auth tests and tell me which ones fail."
export ORION_DICTATION_FILE="$WORK/talk.wav"
