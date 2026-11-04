# 448 — Resting World: stable-ID фундамент и oracle

2 ноября 2026. Исторический архитектурный срез P0-A, не ADR и не принятая оптимизация.

На этом срезе были реализованы и проверены изолированно:

- dense `FragmentId → record/slot` и sparse fallback;
- frozen world boxes и exact irregular shapes;
- persistent cell membership по stable IDs;
- exact `want` gate, ordered dirty IDs и full builder fallback;
- полный и инкрементальный oracle-контакты.

Runtime-интеграция завершена отдельным экспериментом. После исправления точной
moving-refinement семантики matched 200-тиковый бой стал полностью совпадать, однако
persistent путь остался существенно медленнее полного builder. Прототип отклонён и
удалён; рабочее дерево возвращено к ADR-825/826.

Полные замеры, причина отклонения и восстановление baseline:
[449 — Persistent Resting World: точность достигнута, ускорения нет](449-persistent-resting-world-rejected.md).
