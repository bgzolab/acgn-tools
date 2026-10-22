<script setup lang="ts">
import { computed } from "vue";

import { displayTitle, formatPrice, reviewColor, reviewLabelZh, secondaryTitle } from "../composables/format";
import type { GameWithEntry } from "../api/types";
import AccountAvatar from "./AccountAvatar.vue";
import PosterImage from "./PosterImage.vue";
import StatusBadge from "./StatusBadge.vue";

const props = defineProps<{
  item: GameWithEntry;
  /** 批量管理模式：点卡片 = 选中/取消，而不是打开详情 */
  selectable?: boolean;
  selected?: boolean;
  /** 当前按账号筛选时，价格用该账号的区域价格 */
  accountId?: number | null;
}>();
const emit = defineEmits<{
  open: [id: number];
  favorite: [item: GameWithEntry];
  select: [id: number];
}>();

const game = computed(() => props.item.game);
const entry = computed(() => props.item.entry);
const title = computed(() => displayTitle(game.value));
const subtitle = computed(() => secondaryTitle(game.value));

const accounts = computed(() => props.item.accounts ?? []);
const activeAccount = computed(() => {
  const id = props.accountId;
  if (id === null || id === undefined) return null;
  return accounts.value.find((account) => account.account_id === id) ?? null;
});
const accountTooltip = computed(() =>
  accounts.value
    .map((account) => account.display_name?.trim() || account.external_id)
    .join(" / "),
);

// 统一走 formatPrice：自带币种符号、保留两位小数，避免自己实现一份。
// 按账号筛选时优先用该账号区域的价格；`price_updated_at` 非空表示「这个区域问过
// Steam 了」——此时仍没有价格就是该区域确实没有售价，不再回退到全局价格。
const discount = computed(() => {
  const account = activeAccount.value;
  if (account?.price_updated_at) return account.discount_percent ?? 0;
  return game.value.discount_percent ?? 0;
});
const priceText = computed(() => {
  const account = activeAccount.value;
  if (account?.price_updated_at) {
    if (account.is_free) return "免费";
    if (account.price_final === null && account.price_initial === null) return "该区无价";
    return formatPrice(account.price_final ?? account.price_initial, account.price_currency);
  }
  if (game.value.is_free) return "免费";
  return formatPrice(game.value.price_final ?? game.value.price_initial, game.value.price_currency);
});

function onCardClick() {
  if (props.selectable) emit("select", game.value.id);
  else emit("open", game.value.id);
}
</script>

<template>
  <article
    class="game-card"
    :class="{ selectable: props.selectable, selected: props.selectable && props.selected }"
    @click="onCardClick"
  >
    <div class="poster">
      <PosterImage
        :src="game.poster_url"
        :fallback-src="game.cover_url"
        :alt="title"
      />
      <div class="corner left">
        <StatusBadge :status="entry.status" />
      </div>
      <div class="corner right">
        <span v-if="props.selectable" class="pick" :class="{ on: props.selected }">
          {{ props.selected ? "✓" : "" }}
        </span>
        <button
          v-else
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
      <span v-if="accounts.length" class="card-accounts" :title="accountTooltip">
        <AccountAvatar
          v-for="account in accounts.slice(0, 3)"
          :key="account.account_id"
          :src="account.avatar_url"
          :name="account.display_name?.trim() || account.external_id"
          :size="16"
        />
        <span v-if="accounts.length > 3" class="hint">+{{ accounts.length - 3 }}</span>
      </span>
      <span v-if="discount > 0" class="badge discount">-{{ discount }}%</span>
      <span v-else class="price">{{ priceText }}</span>
    </div>
  </article>
</template>
