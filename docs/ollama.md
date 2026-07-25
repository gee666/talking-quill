# Ollama guide

1. Install and start Ollama separately from Talking Quill.
2. Pull a text model, for example with `ollama pull llama3.2`.
3. In Settings → Smart processing select Ollama, keep the loopback endpoint unless Ollama is intentionally on your LAN, refresh models, and run Connection test.
4. Select the model. For On-Screen Awareness, use a vision-capable Ollama model; Talking Quill checks live model metadata before enabling the option.

Ollama data is owned by Ollama. Delete history, Reset all application data, and the Talking Quill uninstaller never alter Ollama or its models. LAN Ollama traffic is not end-to-end encrypted by Talking Quill; use a trusted network and endpoint controls. See troubleshooting if the service is unavailable or a model is not installed.
