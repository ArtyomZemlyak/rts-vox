# 449 — Persistent Resting World: точность достигнута, ускорения нет

2 ноября 2026. Артефакты: `out/optimization-449/`. Продолжение направления
[436](436-global-resting-world.md) и фундамента [448](448-resting-world-foundation.md).

## Результат

Прототип persistent `Settled` **отклонён и полностью удалён из рабочего дерева**.
Принятые ADR-825/826 восстановлены. Никакого улучшения тика/FPS от этого эксперимента
в суммарный результат не добавляется.

## Корректность

Первые расхождения были локализованы до фактического `support/from_above` запроса.
Persistent путь применял refined moving shape всегда, а старый builder — только при
`asks || selected`. Для фрагмента 4588 старый `moving=None`, новый — `Some`, из-за
чего исчезала опора. Воспроизведение прежнего условия refinement устранило различие.

После исправления:

- пошаговый oracle реальных motion/support/swept запросов прошёл тики 0–77 на
  physics 1 и 4;
- release World AI-vs-AI, 200 тиков, physics4/navigation8/Rayon4: все 35
  нетайминговых CSV-полей каждого тика совпали, mismatches=0;
- endpoint hash совпал: `0ad7faff4852df79`;
- debug-oracle, hardcoded diagnostics и временные изменения штатного теста удалены.

Это доказывает корректность короткого matched сценария, но не является основанием
для принятия при отрицательной производительности.

## Производительность

Один и тот же baseline и 200-тиковый сценарий:

| Вариант | Mean tick | `debris` | Комментарий |
|---|---:|---:|---|
| ADR-825/826 baseline | 11,634 мс | 2,518 мс | контроль |
| Persistent после refinement fix | 16,321 мс | 7,313 мс | точный, медленный |
| + parallel record computation | 16,088 мс | 7,062 мс | небольшой эффект |
| + unchanged-membership skip | 15,376 мс | 6,272 мс | недостаточно |
| + hash-dedup rider candidates | 14,989 мс | 5,946 мс | лучший mean прототипа |
| + exact hull revision keys | 15,009 мс | 5,851 мс | вариация/другая серия |

Лучший прототип всё равно примерно на 29% медленнее по полному тику. Даже после
удаления старого builder стоимость `debris_settled` оставалась несколько миллисекунд.

Раздельный профиль показал:

- reindex и dirty updates — десятки/сотни микросекунд;
- rider candidate gathering — около 2,5–3,5 мс в развитом бою;
- parallel record computation и unchanged-membership skip не устраняют лидера;
- union hull ranges через `SettledQueries` оказался хуже и был откачен;
- exact keys `damage revision + root/segment Frames + turret angles` уменьшили работу
  только частично: позы многих корпусов в активном бою меняются каждый тик.

Полный cache-friendly builder оказался эффективнее комбинации dense records,
persistent maps, dirty tracking и rider invalidation. Меньшая алгоритмическая работа
снова не означает меньшее реальное время.

## Возврат baseline и гигиена

`crates/sim-core/src/burst.rs` восстановлен из чистого ADR-825 исходника, после чего
повторно применён ADR-826. `resting_world.rs`, hull cache keys и все профiling hooks
удалены. В `sim-core` остались только прежние принятые изменения.

Контрольный restored release:

- 0 расхождений по всем 35 нетайминговым полям на 200 тиках;
- mean 11,348 мс, `debris` 2,465 мс — на уровне/лучше контрольного baseline;
- суммарные принятые результаты остаются теми, что измерены в
  [444](444-cumulative-825-measurement.md).

## Что переносится в следующий этап

1. Нельзя считать persistent index выигрышем только по уменьшению counters.
2. Refinement moving shape является частью точной контактной семантики, а не просто
   доступной геометрией записи.
3. Hull rider invalidation может доминировать над всей экономией spatial membership.
4. Следующий join-кандидат должен учитывать только join-eligible transitions, а не
   все flying IDs; применять dirty records одной sorted batch-операцией вместо per-ID
   insertion/binary updates из отклонённого 440.
5. Публичные мутации `fragments` в `range_fracture` и setup/test путях требуют явных
   invalidation points либо полного builder fallback. Журнал только внутри
   `step_fragments_kernel` недостаточен для доказательства точности.
