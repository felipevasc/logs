# Motor de consultas

Arquivos grandes são consultados por um motor colunar embutido (DuckDB), sem servidor e sem nada a configurar. Filtros, busca, contagens, facetas, agrupamentos, histograma, gráficos, Resumo, comparação de períodos e pivô passam a responder em frações de segundo mesmo com milhões de registros.

## Como funciona

- Na **primeira abertura** de cada arquivo, depois da indexação, o aplicativo lê todos os registros como sempre fez e grava uma cópia colunar deles (etapa “Preparando consultas rápidas”). As aberturas seguintes do mesmo arquivo reaproveitam essa cópia.
- A cópia guarda exatamente os valores que o aplicativo mostra: campos, nível, origem, código, mensagem, entidades canônicas (`@user`, `@src_ip`…) e o texto usado pela busca livre. Nomes e descrições do catálogo de códigos são aplicados na hora da consulta, então editar o catálogo não exige reconstrução.
- As comparações dos filtros e da linguagem de busca (contém, igual, regex, CIDR, curingas, números com unidade etc.) são as mesmas funções do aplicativo, chamadas de dentro do motor. Por isso os resultados são idênticos aos do leitor linha a linha.
- O que depende do texto bruto da linha (`_all`, `bruto:`), regras de ameaça e detecções é confirmado registro a registro sobre os candidatos, com o mesmo código de antes.
- Mudar a data/hora de uma fonte ou os campos derivados gera uma nova cópia em segundo plano; enquanto ela não fica pronta, o leitor linha a linha responde.
- Qualquer falha do motor (disco cheio, arquivo removido, erro inesperado) faz a consulta ser respondida pelo leitor linha a linha, apenas mais devagar.

## Máquinas modestas

- A leitura inicial e a preparação usam todos os núcleos menos um, com prioridade abaixo do normal: a interface e os outros programas continuam respondendo.
- O motor usa no máximo 40% da memória instalada e grava em disco resultados intermediários maiores. Com menos de 8 GB de RAM, a preparação usa lotes menores e um único gravador.
- Resumos com mais de 50.000 grupos retornam os maiores e informam quantos ficaram de fora.

## Espaço em disco

As cópias ficam em `%APPDATA%\LogInsight\engine-v1` (Windows) ou `~/LogInsight/engine-v1` e ocupam cerca de um terço do arquivo original (531 MB para um log JSON de 1,95 GB com 3 milhões de registros, preparado em menos de um minuto). O aplicativo:

- só cria uma cópia quando o disco tem livres ao menos 40% do tamanho do arquivo mais 1 GB;
- apaga cópias sem uso há 30 dias e, acima de 8 GB no total, as usadas há mais tempo.

Variáveis de ambiente opcionais:

| Variável | Efeito |
| --- | --- |
| `LOGINSIGHT_ENGINE_DIR` | Pasta das cópias colunares (por exemplo, em outro disco). |
| `LOGINSIGHT_ENGINE=0` | Desliga o motor; tudo volta ao leitor linha a linha. |
