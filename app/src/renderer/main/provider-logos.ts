import anthropic from '../../../assets/provider-logos/anthropic.png';
import apipie from '../../../assets/provider-logos/apipie.png';
import azure from '../../../assets/provider-logos/azure.png';
import bedrock from '../../../assets/provider-logos/bedrock.png';
import cerebras from '../../../assets/provider-logos/cerebras.png';
import cohere from '../../../assets/provider-logos/cohere.png';
import cometapi from '../../../assets/provider-logos/cometapi.png';
import deepseek from '../../../assets/provider-logos/deepseek.png';
import dockerModelRunner from '../../../assets/provider-logos/docker-model-runner.png';
import fireworksai from '../../../assets/provider-logos/fireworksai.jpeg';
import foundry from '../../../assets/provider-logos/foundry-local.png';
import gemini from '../../../assets/provider-logos/gemini.png';
import genericOpenai from '../../../assets/provider-logos/generic-openai.png';
import giteeai from '../../../assets/provider-logos/giteeai.png';
import groq from '../../../assets/provider-logos/groq.png';
import koboldcpp from '../../../assets/provider-logos/koboldcpp.png';
import lemonade from '../../../assets/provider-logos/lemonade.png';
import litellm from '../../../assets/provider-logos/litellm.png';
import lmstudio from '../../../assets/provider-logos/lmstudio.png';
import localai from '../../../assets/provider-logos/localai.jpeg';
import minimax from '../../../assets/provider-logos/minimax.png';
import mistral from '../../../assets/provider-logos/mistral.jpeg';
import moonshotai from '../../../assets/provider-logos/moonshotai.png';
import novita from '../../../assets/provider-logos/novita.png';
import nvidiaNim from '../../../assets/provider-logos/nvidia-nim.png';
import ollama from '../../../assets/provider-logos/ollama.png';
import omlx from '../../../assets/provider-logos/omlx.png';
import openai from '../../../assets/provider-logos/openai.png';
import openrouter from '../../../assets/provider-logos/openrouter.jpeg';
import perplexity from '../../../assets/provider-logos/perplexity.png';
import ppio from '../../../assets/provider-logos/ppio.png';
import pi from '../../../assets/provider-logos/pi.png';
import privatemode from '../../../assets/provider-logos/privatemode.png';
import sambanova from '../../../assets/provider-logos/sambanova.png';
import textgenwebui from '../../../assets/provider-logos/text-generation-webui.png';
import togetherai from '../../../assets/provider-logos/togetherai.png';
import xai from '../../../assets/provider-logos/xai.png';
import zai from '../../../assets/provider-logos/zai.png';
import type { ProviderId } from '../../shared/schemas/providers';

export const PROVIDER_LOGOS = Object.freeze({
  anthropic,
  apipie,
  azure,
  bedrock,
  cerebras,
  cohere,
  cometapi,
  deepseek,
  'docker-model-runner': dockerModelRunner,
  fireworksai,
  foundry,
  gemini,
  'generic-openai': genericOpenai,
  giteeai,
  groq,
  koboldcpp,
  lemonade,
  litellm,
  lmstudio,
  localai,
  minimax,
  mistral,
  moonshotai,
  novita,
  'nvidia-nim': nvidiaNim,
  ollama,
  omlx,
  openai,
  openrouter,
  perplexity,
  ppio,
  pi,
  privatemode,
  sambanova,
  textgenwebui,
  togetherai,
  xai,
  zai,
} satisfies Readonly<Record<ProviderId, string>>);
