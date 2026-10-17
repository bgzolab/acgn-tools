<script setup lang="ts">
import { computed } from "vue";

import { displayTitle, formatPrice, reviewColor, reviewLabelZh, secondaryTitle } from "../composables/format";
import type { GameWithEntry } from "../api/types";
import PosterImage from "./PosterImage.vue";
import StatusBadge from "./StatusBadge.vue";

const props = defineProps<{ item: GameWithEntry }>();
const emit = defineEmits<{ open: [id: number]; favorite: [item: GameWithEntry] }>();

const game = computed(() => props.item.game);
const entry = computed(() => props.item.entry);
const title = computed(() => displayTitle(game.value));
const subtitle = computed(() => secondaryTitle(game.value));
const discount = computed(() => game.value.discount_percent ?? 0);
// 统一走 formatPrice：自带币种符号、保留两位小数，避免自己实现一份
// （$0.49 曾经被截成 $0）
const priceText = computed(() => {
  if (game.value.is_free) return "免费";
  return formatPrice(game.value.price_final ?? game.value.price_initial, game.value.price_currency);
});
</script>

<template>
  <article class="game-card" @click="emit('open', game.id)">
    <div class="poster">
      <PosterImage :src="game.poster_url || game.cover_url" :alt="title" />
      <div class="corner left">
        <StatusBadge :status="entry.status" />
      </div>
      <div class="corner right">
        <button
          class="icon-btn"
          :class="{ on: entry.favorite }"
          :title="entry.favorite ? '取消收藏' : '加入收藏'"
          @click.stop="emit('favorite', props.item)"
        >
          ★
        </button>
      </div>
    </div>

    <div class="title" :title="title">{{ title }}</div>
    <div v-if="subtitle" class="title-en" :title="subtitle">{{ subtitle }}</div>

    <div class="meta">
      <span :style="{ color: reviewColor(game.review_score) }">
        {{ game.review_score !== null ? reviewLabelZh(game) : "待补全" }}
      </span>
      <span v-if="discount > 0" class="badge discount">-{{ discount }}%</span>
      <span v-else class="price">{{ priceText }}</span>
    </div>
  </article>
</template>
