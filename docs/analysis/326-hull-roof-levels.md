# 326. Отвергнутый отбор корпусов по достижимой высоте крыши

Дата: 2026-10-18. База: `main 2e4ca6a`. Активная очередь —
[316](316-optimization-delivery-plan.md), OPT316-04. Два предшествующих
эксперимента, [XZ-индекс корпусов324](324-hull-support-candidates.md)
и [колонки контактных голов325](325-settled-column-heads.md), не ускорили
полный тик и удалены. Их прототипы, бинарники и результаты сохранены.

**Статус: отвергнуто, production код удалён.** Mean6000: before-a16,638,
after-a16,849, свежий reverse before-b16,648 мс. Late root18,810→19,093
при reverse18,794; Settled candidates0,582→0,634 мс. P99:42,644/43,036/
42,515 мс. Все36 игровых полей и hash `21bef878463f364c` совпали.
Польза не подтверждена, дополнительная полная/native квалификация
не проводилась; прежняя production реализация восстановлена.

Диагностический `bench_rubble_snapshot --support-levels` на snapshot6000
с тем же hash показывает39 корпусов и уровни16..38. Среди65013 resting
records кандидатов0 у4476,5 у29160,30 у901,
35 у6500,36 у3510,37 у7150,38 у6069,39 у7247. Простое уменьшение числа
кандидатов не означает меньшую стоимость алгоритма. Распределение
относится ко всем resting, до фильтра запросов и world-floor classify.
Режим диагностики сохранён, не используется в SIM.

## Использованное необходимое условие

Действующий `Hull::carries` для voxel-корпуса первым отвергает запрос,
если `floor(bottom+1/2) < floor(hull.pos.y)+1`. Это первая возможная
крыша в сканируемом диапазоне, независимо от XZ, heading и дыр.
Для корпуса без voxel callback необходимая нижняя граница —
`floor(hull.pos.y + height)`: точный wrapped roof должен быть не выше
probe, следовательно его floor не выше floor probe.

`HullSupportLevels` заранее объединяет эти пороги. Каждая целая высота
probe содержит многословную маску допустимых **source indices**.
Проверка ниже всех порогов возвращает пустой список без обхода корпусов.
Проверка не ниже максимального порога сохраняет полный исходный список.
Между ними перечисляются только установленные биты, в source order.

Оставшийся кандидат проходит прежний `carries`, включая точный roof,
половину единицы, XZ и voxel holes. `Settled` сохраняет и прежний bbox
предикат. Floor для box roof — только необходимое условие, не замена
дробной высоты или новая геометрия. Wrapping Fx-арифметика прежняя.

## Представление и актуальность

Таблица строится заново по нынешнему immutable hull slice для rubble
classify и Settled candidates. Порядок корпусов захватывается вместе
с порогами. Старые позы, крыши и ответы между фазами не переиспользуются.
Persistent fragment index из04a/04b этим не реализован.

Это многословная маска, а не ограничение64 корпусов. Технические пределы
таблицы:256 целых уровней и4096 u64 записей, до32 КиБ логического payload.
Если диапазон/число слов больше, сохраняется полный исходный перебор.
Работа, контакты, число юнитов и высота мира не ограничиваются.
Временный Vec порогов линейный по hull count; аллокации и построение
входят в parent timings. Снижение RSS/VRAM не заявляется.

Build rubble входит в `physics_rubble_prepare`, build Settled — в
candidate interval. Вложенные spans не суммируются с parent/root.
Частота SIM, logical quotas, материя, оружие, swept reach, same-tick
геометрия/навигация и формат save/hash сохраняются.

## Проверки и стенд

Независимый тест сравнивает полные списки успешных source IDs по двум
исходным предикатам: `carries` и bbox+`carries`. Входы0/1/7/65/257,
дробные и отрицательные высоты, изменение поз/порядка/heading, roof holes,
точки MIN/MAX Fx и wrapped box roofs. Отдельный fixture5000 проверяет
многословный путь и точный fallback при превышении memory gate.
Поддерживаемые и неподдерживаемые точки обязательны.

Fixed-work baseline — frozen `out/optimization-323/after-*`. Runtime inputs
497, изменён `burst.rs`. Перед release probe прошли debug адресный oracle
и14 rubble tests. Before-a перенесён из324: это тот же baseline binary,
World AIvsAI6000,physics4/nav8/Rayon4, без изменения CSV/log.
Он предшествует отвергнутым экспериментам; это не новая соседняя A/B пара.
Остальные проверки квалифицируются отдельно. На каждом тике сравниваются
все36 нетайминговых полей и terminal hash.

## Артефакты и границы

Локально `out/optimization-326/`: source/data fingerprints, binaries,
CSV/log и анализаторы. Evidence описывает source/binary content,
не аттестует чистую сборку или toolchain.

```powershell
$env:RAYON_NUM_THREADS='4'
./out/optimization-323/after-bench_battle.exe --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --profile-rubble --tick-csv out/optimization-326/repeat-before.csv
./out/optimization-326/after-bench_battle.exe --world --ai-vs-ai --ticks 6000 --physics-workers 4 --workers 8 --profile-rubble --tick-csv out/optimization-326/repeat-after.csv
```

5000 здесь — адресная корректность фильтра, не доказанная производительность
массового боя. Полный drain валки, persistent membership, every-tick≤33 мс,
native165 FPS, редкие секундные фризы и долгий memory/VRAM soak остаются
отдельными открытыми пунктами единого плана.
