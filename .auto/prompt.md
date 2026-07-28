# Autoresearch objective

Only optimize/prove this exact chain:

1. zero simulated-button autoload of slot 0 / active default character;
2. same character reloads once;
3. same character reloads a second time;
4. each load reaches an objective movement semaphore;
5. playable-window framerate is stable across the three loads.

Do not chase menu screenshots, cosmetic title-cover states, or alternate-character/cross-save cases. A run is good only if telemetry proves epochs 0, 1, and 2 for the same character, `simulated_button_presses_total == 0`, no message boxes, movement semaphores for all three loads, and playable FPS parity.
