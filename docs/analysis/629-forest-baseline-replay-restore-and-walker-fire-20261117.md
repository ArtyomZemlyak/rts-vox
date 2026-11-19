# 629 — forest baseline: replay/restore и штатный выстрел

Статус: **приёмочный дефект текущего627 исследуется и исправляется; не готово**.
Пакет распыления листвы не читается и не интегрируется. Производительный
пункт628 остаётся в прежнем порядке. Номер629 — номер отчёта о выявленном
дефекте незавершённой приёмки627, не перестановка очереди Codex.

## Сохранённый baseline пользователя

`out/kimi-forest-baseline-before-dust/test-index.json` подтверждает source
`6179950ff17ca347e6495910e18acf14b39eea9aec228b3f3d5d695a98bfc038`,
0passed/3failed/0ignored, exit101. Исходный лог:
`out/kimi-forest-baseline-before-dust/sim-core-lib-tests.log`.

| Исходная проверка | Результат на пользовательском baseline |
|---|---|
| spider_contact_fells_trunk_and_crown_on_both_sides | 16042963605631056541 != 6188163134628311400, workers/replay |
| tall_tree_crowns_above_the_felling_radius_collapse_after_walker_contact | 13837695431231164226 != 10359527480904716367 |
| every_armed_kind_damages_wood_with_its_stock_weapon | Walker04 never fired за600ticks |

SHA256 исходного test-index:
`ebcaee6ac4db7be1429914a5d3922644dea215ea72c829b0d2eb9eb46c16ff9c`;
лога: `20d825de61e2be1fe6c43ebeb544fba2754ad4a9267e9be9306ceb40e523c18e`.
Оригиналы не изменены. Это отдельное доказательство, не относящееся к
`out/kimi-forest-dust-workspace` и не приписываемое его isolated-патчу.

## Свежая проверка фактического main до исправления

В main source был
`c4356e47900578dad9652a0694107f6ab4b3dd5b2e985030409aa4e1e2d56776`.
Выполнены ровно три исходных теста на verified cached harness через
scripts/test-fast.py --no-build с тем же оптимизированным профилем G.
Результат: **2passed/1failed/0ignored**,9,681s.

- Spider workers/replay прошёл без изменений теста.
- Высокая крона/restore прошла без изменений теста.
- Walker04 never fired воспроизвёлся за исходные600ticks.

Receipts: `out/forest-baseline629/test-index.json`, `provenance.json`,
`sim-core-lib-tests.log`. Тесты и source были неизменны во время этого прогона.
Это доказывает текущие две проверки, но не является объяснением всех причин
старого6179950f десинка и не выдаётся за новый фикс этих двух ошибок.

Доставка627 заблокирована. Старое qualification сохранено как
`out/integration627g/qualification-before-forest-baseline629.json`, текущий
допуск механики отозван. «Полигон» остаётся626.488старых зелёных проверок
не используются для сокрытия вновь воспроизведённого red.

## Найденная причина Walker04 и исправление

Согласованный ствол ещё до laser_ray проверял точку по column_ceiling,
включавшему древесину/крону, и требовал свободную LOS к внутренней точке
древесины. Явный приказ стрелять в дерево поэтому отклонялся до физического
контакта, в отличие от обычной политики явных point orders.

world_target_clear различает автоматический выбор и явный point order.
Автоматический путь сохраняет прежние ceiling/LOS. Явный путь использует
существующий data-defined yielding-профиль навигации для точного пола одной
колонки и по-прежнему отвергает грунт/твёрдую преграду. Растительность
допускается до физического луча; луч не пропускает древесину, а повреждает
первый реальный материал обычным кодом. Материя, оружейные/серво-пороги,
наведение, reload/charge, физические опоры и порядок валки не изменены.

Первая исполненная версия дошла в исходном all-armed тесте до
`Walker04 fired=41 wood=9 burned=9`. После этого выявился ранее недостигнутый
отказ restore у следующего вооружённого типа; он исследуется отдельно,
исходный unwrap/restore и проверка выстрела не удалены.

## Дополнительные проверки и найденные отказы

Новый модуль `range_walker_world_target_tests.rs` проверяет физическую
древесину и полость под кроной, data-profile classification, автоматическую
видимость, грунт, твёрдую преграду, границы мира. Реальный Walker04 выполняет
120ticks для W1/2/4/8; cold restore сделан во время charge и после первого
выстрела с другим числом workers. Сравниваются все потактовые hashes,
неподвижность позиции, реальное повреждение древесины, отсутствие подмены
выстрела contact felling и баланс материи.

Эти новые проверки прошли в диагностическом `fix-fixture4`; весь этот прогон
**не квалифицируется**, поскольку во время его выполнения агент изменил
отдельный устаревший счётчик test roster. Нужен свежий единый source.
Исходные три тестовых тела неизменны:
`out/forest-baseline629/original-test-contracts.json`.

Отдельный старый emplacement-тест уже проверял15типов, но ожидал12.
Исправлено ожидание15 с явным учётом существующих PowerPlantT1/T2/T3;
все проверки неподвижности/запрета удалённой валки сохранены. Это не
физический порог и не адаптация к распылению листвы.

Все промежуточные отказы сохранены:

- fix-first: ошибка доступа нового тестового модуля к приватным Actor-полям;
  модуль перенесён под battle, видимость полей не расширялась.
- fix-compile2: ошибочный размер моего нового fixture и вновь достигнутый
  restore-отказ после успешного выстрела Walker04; старый roster12!=15.
- fix-fixture3: неверный тип optional unit в диагностическом observer,
  исправлен только новый тест.
- fix-fixture4:13passed/2failed; новый Walker04 cold/workers passed, но source
  изменён во время работы, поэтому ни один результат не является допуском.
- commander-small5: воспроизведён отказ первой стрельбы командира в исходной
  малой геометрии. Законная offhand servo-точка [316,-155,224] отвергалась
  прежней границей Y=-128. Последний observer был wave_contacts, поскольку
  поздняя секция walker_poses не имела собственной отметки.

## Дополнительно найденный дефект Commander cold restore

В исходном all-armed тесте после исправления выстрела Walker04 выполнение
впервые дошло до выстрела Commander. На карте768×128×768 снимок сохранял
интерполированную точку свободной руки [316,-155,224], а загрузчик проверял
её как world point с допустимым Y не ниже -128. Серво действительно может
повернуть горизонтальный радиус в вертикальную ось; это корректная поза,
а не повреждение сохранения.

range_walker_save.rs теперь использует только для Commander конечный
геометрический запас sum(world axes)+data rest_lookahead_cm по каждой оси.
Walker04 сохраняет старую границу; ID/order/count/kind/arm metadata и Scan
проверки сохранены. Движение, поза, байтовый формат и физические пороги
не меняются. В restore observer добавлена отметка walker_poses.
Новая проверка воспроизводит исходную геометрию и требует первого выстрела,
полного cold restore и32 потактовых продолжений. Отдельная проверка
загрузчика отвергает i32::MIN/MAX и недопустимую metadata руки.

Стабильный source для новых прогонов фиксируется в
`out/forest-baseline629/fix-source.json`; старые G proofs к нему не относятся.

## Стандартный профиль пользовательского прогона

Исходный пользовательский test-index имеет build_environment={} — стандартные
debug assertions=true. Они отдельно проверены: **3/3,0failed/ignored,38,587s**,
build182,85s. Compile source640b0b1c4171bc932396e6a13b81e7f15f5bf83cb6afbe2d419348aae80445b6.
Receipt: out/forest-baseline629/default-profile-current/test-index.json.
904файла этого source, включая docs/07-economy.md, сохранены в default-profile-source.zip, точный compile
hash проверен при создании архива; receipt default-profile-source.json.
Он включает отдельный незавершённый кандидат630, поэтому не является
полной интеграционной квалификацией. Старые два hash-десинка6179 на
нынешних проверках не воспроизведены; их причина не приписана новому
point-fire/codec исправлению.

## Итоговая проверка одного неизменного исходника

Compile source: `ddb23e19144076c4bc96f88c473e8438ab032ad83c6e58dfd8e1ac222d064336`.
Expanded903: `97bd107495d66a996d48a6217c1108a17ed7e3faa4c7f31aff40655c73c588ab`.
Повторный verify-fix-source подтвердил идентичность входов до/после проверок.

| Проверка | Результат | Receipt |
|---|---|---|
| Ровно3 исходных теста | 3/3,0failed/ignored;38,244s | out/forest-baseline629/original-three-final/test-index.json |
| Расширенный forest/приказы/заряд/cold/worker набор |16/16,0failed/ignored;161,093s | out/forest-baseline629/focused-final7/test-index.json |
| Архитектура |12/12 |out/forest-baseline629/architecture-final/test-index.json |
| Документация |3/3 |out/forest-baseline629/docs-final/test-index.json |
| Дополнительный Commander sweep |51/54,3failed |out/forest-baseline629/commander-codec6/test-index.json |

Наборы пересекаются; исходные3 входят в расширенные16. Их полные тексты
сравнены с source-before: final-test-contracts.json. Собственный diff
от исходной рабочей версии сохранён в implementation.patch, включая codec,
observer и новые тесты. Код isolated-пакета листвы не использован.

## Done / remaining

| Done | Remaining |
|---|---|
| Историческое0/3 сохранено; актуальный main до правки2/3 | Старые два hash-десинка6179 на актуальном main не воспроизведены; их причина не приписана этому патчу |
| Walker04 point-fire исправлен с настоящим wood damage и исходным600tick контрактом | Производительность628 остаётся отдельной очередной задачей |
| Первая стрельба Commander cold restores; защита codec от повреждённых координат/metadata проверена | Дополнительные3red Commander: ремонт и2походки — конкретные сценарии в additional-red-commander.json, продолжаются в рамках627 |
| Полный узкий набор16/16 и архитектура/документы15/15 на одном source | Общая повторная интеграционная приёмка627 и доставка на новом source пока не выполнены; G не подменяет её |

Нового release и обновления «Полигона» не выполняли. Физические пороги,
replay/restore/workers/arrival/shot assertions не удаляли и не ослабляли.

## Повторная интеграционная проверка J

Текущий frozen J92eee063 прошёл544/544 test-fast, включая все исходные
forest3, новые point-fire/cold/workers проверки и ранее красные Commander
сценарии. Отдельный стандартный профиль на том же J также прошёл3/3,
0failed/ignored,40,509s;build102,67s. Receipt:
out/integration627j/forest-default/test-index.json, build_environment={}.
Исторический default-profile-current не переименовывается в текущий допуск.
Receipt:out/integration627j/checks/test-index.json.

J дополнил544/544 иdefault3/3 сквозной механикой17/17,12аудитами,
Spider continuation/sweep,боем2400W1/2+cold300 иполным равновесием воды.
Native/release/боевой A/B J ещё впереди;canonical626необновлён.
