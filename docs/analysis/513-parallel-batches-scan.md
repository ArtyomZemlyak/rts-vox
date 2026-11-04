# 513 — Parallel Batches stamp scan

4 ноября 2026. Артефакты: `out/optimization-513/`.

## Причина

После ADR-840/841 `Batches::update` всё ещё последовательно читает всю retained
history. В позднем бою это сотни тысяч фрагментов на кадр, хотя mesh/ECS mutation
начинается только после read-only stamp scan.

Проба442 не является противопоказанием для этого варианта: там per-item Rayon collect
замедлил join current на51,91%. Здесь используются крупные независимые source ranges,
по 16384 фрагмента, и существующий ComputeTaskPool.

## Прототип

Каждый chunk начинает старый input cursor через `partition_point`, вычисляет прежние
stamps и dirty tiles и пишет в переиспользуемый buffer. Главный поток объединяет buffers
в исходном порядке и объединяет dirty BTreeSets. Дальнейшие tile reconciliation,
publication budget, meshes, ECS commands и flags не меняются.

Serial-путь сохранён для малых inputs и pools менее4 workers. Параллельный scan
включается от двух полных chunks. Никаких изменений SIM или snapshot state нет.

## Проверки и результат

Прямой serial/parallel test прошёл: stamps и dirty set совпали после rotation, removal,
wake, geometry edits и landing interpolation. Обязательный lifecycle-тест также прошёл.

Четыре 60-секундных World AI-vs-AI capture AB/BA против ADR-842, все валидны:

| Пара | Mean FPS | p99 кадра | `fragments` |
|---|---:|---:|---:|
|1, after→before|33,638→32,839 (+2,44%)|92,393→91,938 мс|7,623→7,419 мс|
|2, before→after|33,319→32,680 (−1,92%)|90,412→91,974 мс|7,408→7,556 мс|

Pooled FPS33,079→33,159 (+0,24%), но `fragments`7,413→7,589 мс (**+2,38%**),
p99 кадра91,175→92,184 мс (**+1,11%**). Знаки FPS по парам разные, фаза и хвост
ухудшились. Ordered copy и сборка chunk results не окупили parallel scan.

## Решение

Эксперимент **отклонён и полностью откачён**. Chunk buffers, parallel scan и его test
удалены; serial ADR-840 и принятые ADR-841/842 сохранены. Новый ADR не создаётся.
